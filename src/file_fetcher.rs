use futures_util::StreamExt;
use reqwest::{
    header::{ACCEPT_ENCODING, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE},
    Client, StatusCode,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const RESUME_METADATA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct FileProgress {
    pub downloaded: u64,
    pub total: u64,
    pub resumed: bool,
    pub resume_from: u64,
    pub attempt: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct ResumeMetadata {
    #[serde(default)]
    version: u32,
    url: String,
    expected_total: Option<u64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

impl ResumeMetadata {
    fn from_headers(
        url: &str,
        expected_total: Option<u64>,
        headers: &reqwest::header::HeaderMap,
    ) -> Self {
        Self {
            version: RESUME_METADATA_VERSION,
            url: url.to_string(),
            expected_total,
            etag: header_string(headers, ETAG),
            last_modified: header_string(headers, LAST_MODIFIED),
        }
    }

    fn matches_response_headers(&self, headers: &reqwest::header::HeaderMap) -> bool {
        header_matches(&self.etag, headers, ETAG)
            && header_matches(&self.last_modified, headers, LAST_MODIFIED)
    }
}

pub async fn download_file(client: &Client, url: &str, dest: &Path) -> anyhow::Result<()> {
    download_file_with_progress(client, url, dest, |_| {}).await
}

pub async fn content_length(client: &Client, url: &str) -> anyhow::Result<Option<u64>> {
    let resp = client
        .head(url)
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.content_length())
}

pub async fn download_file_with_progress<F>(
    client: &Client,
    url: &str,
    dest: &Path,
    mut on_progress: F,
) -> anyhow::Result<()>
where
    F: FnMut(FileProgress),
{
    let max_retries = 6;
    let mut attempt = 0;
    let temp_dest = temp_download_path(dest);
    let metadata_dest = resume_metadata_path(&temp_dest);

    loop {
        attempt += 1;
        let result = try_download_with_progress(
            client,
            url,
            dest,
            &temp_dest,
            &metadata_dest,
            attempt,
            &mut on_progress,
        )
        .await;

        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                if attempt >= max_retries {
                    return Err(e);
                }

                let delay_ms = (750 * attempt as u64).min(5_000);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }
    }
}

async fn try_download_with_progress<F>(
    client: &Client,
    url: &str,
    dest: &Path,
    temp_dest: &Path,
    metadata_dest: &Path,
    attempt: u32,
    on_progress: &mut F,
) -> anyhow::Result<()>
where
    F: FnMut(FileProgress),
{
    let metadata = load_resume_metadata(metadata_dest).await;
    let mut resume_from = tokio::fs::metadata(temp_dest)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    if resume_from > 0 {
        match metadata.as_ref() {
            Some(metadata)
                if metadata.url == url && metadata.version == RESUME_METADATA_VERSION =>
            {
                if metadata
                    .expected_total
                    .is_some_and(|expected_total| resume_from > expected_total)
                {
                    reset_partial_download(temp_dest, metadata_dest).await;
                    resume_from = 0;
                }
            }
            Some(_) => {
                reset_partial_download(temp_dest, metadata_dest).await;
                resume_from = 0;
            }
            None => {}
        }
    }

    let requested_resume_from = resume_from;
    let mut request = client.get(url).header(ACCEPT_ENCODING, "identity");
    if resume_from > 0 {
        request = request.header(RANGE, format!("bytes={}-", resume_from));
    }

    let resp = request.send().await?;
    if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        reset_partial_download(temp_dest, metadata_dest).await;
        anyhow::bail!("server rejected resume range");
    }

    let status = resp.status();
    let headers = resp.headers().clone();
    let resp = resp.error_for_status()?;

    if resume_from > 0 && status != StatusCode::PARTIAL_CONTENT {
        reset_partial_download(temp_dest, metadata_dest).await;
        resume_from = 0;
    } else if resume_from > 0
        && metadata
            .as_ref()
            .is_some_and(|metadata| !metadata.matches_response_headers(&headers))
    {
        reset_partial_download(temp_dest, metadata_dest).await;
        anyhow::bail!("remote file changed since partial download was created");
    }

    let response_len = resp.content_length().unwrap_or(0);
    let total_size = if status == StatusCode::PARTIAL_CONTENT {
        parse_content_range_total(&headers).unwrap_or(resume_from + response_len)
    } else {
        response_len
    };
    let expected_total = (total_size > 0).then_some(total_size);
    save_resume_metadata(
        metadata_dest,
        &ResumeMetadata::from_headers(url, expected_total, &headers),
    )
    .await?;

    let mut file = if resume_from > 0 {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(temp_dest)
            .await?
    } else {
        tokio::fs::File::create(temp_dest).await?
    };
    let mut stream = resp.bytes_stream();
    let mut downloaded = resume_from;
    let mut last_update = std::time::Instant::now();
    let mut last_downloaded = downloaded;

    if downloaded > 0 {
        on_progress(FileProgress {
            downloaded,
            total: total_size,
            resumed: true,
            resume_from: downloaded,
            attempt,
        });
    }

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let chunk_len = chunk.len() as u64;
        file.write_all(&chunk).await?;
        downloaded += chunk_len;

        let now = std::time::Instant::now();
        let elapsed = now.duration_since(last_update).as_millis();
        let bytes_since = downloaded.saturating_sub(last_downloaded);

        if elapsed >= 500 || bytes_since >= 1024 * 1024 || last_downloaded == resume_from {
            on_progress(FileProgress {
                downloaded,
                total: total_size,
                resumed: resume_from > 0,
                resume_from: requested_resume_from,
                attempt,
            });
            last_update = now;
            last_downloaded = downloaded;
        }
    }

    file.flush().await?;
    drop(file);

    if total_size > 0 && downloaded != total_size {
        anyhow::bail!(
            "incomplete download: received {} of {} bytes",
            downloaded,
            total_size
        );
    }

    tokio::fs::rename(temp_dest, dest).await?;
    let _ = tokio::fs::remove_file(metadata_dest).await;
    on_progress(FileProgress {
        downloaded,
        total: total_size,
        resumed: resume_from > 0,
        resume_from: requested_resume_from,
        attempt,
    });
    Ok(())
}

async fn load_resume_metadata(path: &Path) -> Option<ResumeMetadata> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&content).ok()
}

async fn save_resume_metadata(path: &Path, metadata: &ResumeMetadata) -> anyhow::Result<()> {
    let content = serde_json::to_vec(metadata)?;
    tokio::fs::write(path, content).await?;
    Ok(())
}

async fn reset_partial_download(temp_dest: &Path, metadata_dest: &Path) {
    let _ = tokio::fs::remove_file(temp_dest).await;
    let _ = tokio::fs::remove_file(metadata_dest).await;
}

fn parse_content_range_total(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers.get(CONTENT_RANGE)?.to_str().ok()?;
    let total = value.rsplit_once('/')?.1;
    if total == "*" {
        None
    } else {
        total.parse().ok()
    }
}

fn header_string(
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn header_matches(
    expected: &Option<String>,
    headers: &reqwest::header::HeaderMap,
    name: reqwest::header::HeaderName,
) -> bool {
    match expected {
        Some(expected) => headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|actual| actual == expected),
        None => true,
    }
}

fn temp_download_path(dest: &Path) -> PathBuf {
    let file_name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    dest.with_file_name(format!("{}.part", file_name))
}

fn resume_metadata_path(temp_dest: &Path) -> PathBuf {
    let file_name = temp_dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download.part".to_string());
    temp_dest.with_file_name(format!("{}.meta", file_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::thread;

    #[test]
    fn parse_content_range_total_reads_known_total() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 10-99/200"));

        assert_eq!(parse_content_range_total(&headers), Some(200));
    }

    #[test]
    fn parse_content_range_total_ignores_unknown_total() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 10-99/*"));

        assert_eq!(parse_content_range_total(&headers), None);
    }

    #[test]
    fn temp_download_path_appends_part_suffix() {
        let path = Path::new("music/song.wav");

        assert_eq!(temp_download_path(path), Path::new("music/song.wav.part"));
    }

    #[test]
    fn resume_metadata_path_appends_meta_suffix() {
        let path = Path::new("music/song.wav.part");

        assert_eq!(
            resume_metadata_path(path),
            Path::new("music/song.wav.part.meta")
        );
    }

    #[tokio::test]
    async fn download_file_with_progress_resumes_partial_file() {
        let body = b"abcdef".to_vec();
        let server = TestServer::spawn({
            let body = body.clone();
            move |request, _| {
                let range = request
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(str::trim)
                            .map(str::to_string)
                    })
                    .and_then(|value| value.strip_suffix('-').map(str::to_string))
                    .and_then(|value| value.parse::<usize>().ok());
                match range {
                    Some(start) => Response::partial(&body[start..], start, body.len(), "v1"),
                    None => Response::ok(&body, "v1"),
                }
            }
        });
        let root = test_dir("resume");
        let dest = root.join("song.bin");
        let temp_dest = temp_download_path(&dest);
        let metadata_dest = resume_metadata_path(&temp_dest);
        std::fs::write(&temp_dest, b"abc").unwrap();
        std::fs::write(
            &metadata_dest,
            serde_json::to_vec(&ResumeMetadata {
                version: RESUME_METADATA_VERSION,
                url: server.url(),
                expected_total: Some(body.len() as u64),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
            })
            .unwrap(),
        )
        .unwrap();

        let mut progress = Vec::new();
        download_file_with_progress(&reqwest::Client::new(), &server.url(), &dest, |item| {
            progress.push(item);
        })
        .await
        .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!temp_dest.exists());
        assert!(!metadata_dest.exists());
        assert!(progress
            .iter()
            .any(|item| item.resumed && item.resume_from == 3));
    }

    #[tokio::test]
    async fn download_file_with_progress_restarts_when_server_ignores_range() {
        let body = b"abcdef".to_vec();
        let server = TestServer::spawn({
            let body = body.clone();
            move |_request, _| Response::ok(&body, "v1")
        });
        let root = test_dir("ignore-range");
        let dest = root.join("song.bin");
        let temp_dest = temp_download_path(&dest);
        let metadata_dest = resume_metadata_path(&temp_dest);
        std::fs::write(&temp_dest, b"abc").unwrap();
        std::fs::write(
            &metadata_dest,
            serde_json::to_vec(&ResumeMetadata {
                version: RESUME_METADATA_VERSION,
                url: server.url(),
                expected_total: Some(body.len() as u64),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
            })
            .unwrap(),
        )
        .unwrap();

        download_file_with_progress(&reqwest::Client::new(), &server.url(), &dest, |_| {})
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[tokio::test]
    async fn download_file_with_progress_restarts_when_metadata_url_differs() {
        let body = b"abcdef".to_vec();
        let server = TestServer::spawn({
            let body = body.clone();
            move |request, _| {
                assert!(!request.to_ascii_lowercase().contains("range: bytes=3-"));
                Response::ok(&body, "v1")
            }
        });
        let root = test_dir("url-change");
        let dest = root.join("song.bin");
        let temp_dest = temp_download_path(&dest);
        let metadata_dest = resume_metadata_path(&temp_dest);
        std::fs::write(&temp_dest, b"abc").unwrap();
        std::fs::write(
            &metadata_dest,
            serde_json::to_vec(&ResumeMetadata {
                version: RESUME_METADATA_VERSION,
                url: "http://127.0.0.1:1/old".to_string(),
                expected_total: Some(body.len() as u64),
                etag: Some("\"v1\"".to_string()),
                last_modified: None,
            })
            .unwrap(),
        )
        .unwrap();

        download_file_with_progress(&reqwest::Client::new(), &server.url(), &dest, |_| {})
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[tokio::test]
    async fn download_file_with_progress_restarts_when_metadata_version_differs() {
        let body = b"abcdef".to_vec();
        let server = TestServer::spawn({
            let body = body.clone();
            move |request, _| {
                assert!(!request.to_ascii_lowercase().contains("range: bytes=3-"));
                Response::ok(&body, "v1")
            }
        });
        let root = test_dir("metadata-version");
        let dest = root.join("song.bin");
        let temp_dest = temp_download_path(&dest);
        let metadata_dest = resume_metadata_path(&temp_dest);
        std::fs::write(&temp_dest, b"abc").unwrap();
        std::fs::write(
            &metadata_dest,
            serde_json::json!({
                "version": 0,
                "url": server.url(),
                "expected_total": body.len(),
                "etag": "\"v1\"",
                "last_modified": null,
            })
            .to_string(),
        )
        .unwrap();

        download_file_with_progress(&reqwest::Client::new(), &server.url(), &dest, |_| {})
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!temp_dest.exists());
        assert!(!metadata_dest.exists());
    }

    #[tokio::test]
    async fn download_file_with_progress_retries_after_short_response() {
        let body = b"abcdef".to_vec();
        let server = TestServer::spawn({
            let body = body.clone();
            move |_request, count| {
                if count == 1 {
                    Response {
                        status: "HTTP/1.1 200 OK".to_string(),
                        headers: vec![
                            ("Content-Length".to_string(), body.len().to_string()),
                            ("ETag".to_string(), "\"v1\"".to_string()),
                        ],
                        body: b"abc".to_vec(),
                    }
                } else {
                    Response::ok(&body, "v1")
                }
            }
        });
        let root = test_dir("retry");
        let dest = root.join("song.bin");

        download_file_with_progress(&reqwest::Client::new(), &server.url(), &dest, |_| {})
            .await
            .unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(server.request_count() >= 2);
    }

    struct TestServer {
        addr: std::net::SocketAddr,
        request_count: Arc<AtomicUsize>,
    }

    impl TestServer {
        fn spawn<F>(handler: F) -> Self
        where
            F: Fn(String, usize) -> Response + Send + Sync + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let handler = Arc::new(handler);
            let request_count = Arc::new(AtomicUsize::new(0));
            let request_count_for_thread = request_count.clone();

            thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    let count = request_count_for_thread.fetch_add(1, Ordering::SeqCst) + 1;
                    let request = read_request(&mut stream);
                    let response = handler(request, count);
                    response.write_to(&mut stream);
                }
            });

            Self {
                addr,
                request_count,
            }
        }

        fn url(&self) -> String {
            format!("http://{}/song.bin", self.addr)
        }

        fn request_count(&self) -> usize {
            self.request_count.load(Ordering::SeqCst)
        }
    }

    struct Response {
        status: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Response {
        fn ok(body: &[u8], etag: &str) -> Self {
            Self {
                status: "HTTP/1.1 200 OK".to_string(),
                headers: vec![
                    ("Content-Length".to_string(), body.len().to_string()),
                    ("ETag".to_string(), format!("\"{}\"", etag)),
                ],
                body: body.to_vec(),
            }
        }

        fn partial(body: &[u8], start: usize, total: usize, etag: &str) -> Self {
            Self {
                status: "HTTP/1.1 206 Partial Content".to_string(),
                headers: vec![
                    ("Content-Length".to_string(), body.len().to_string()),
                    (
                        "Content-Range".to_string(),
                        format!("bytes {}-{}/{}", start, total - 1, total),
                    ),
                    ("ETag".to_string(), format!("\"{}\"", etag)),
                ],
                body: body.to_vec(),
            }
        }

        fn write_to(self, stream: &mut TcpStream) {
            let mut response = format!("{}\r\n", self.status);
            for (name, value) in self.headers {
                response.push_str(&format!("{}: {}\r\n", name, value));
            }
            response.push_str("Connection: close\r\n\r\n");
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&self.body).unwrap();
        }
    }

    fn read_request(stream: &mut TcpStream) -> String {
        let mut buf = [0; 4096];
        let mut request = Vec::new();
        loop {
            let len = stream.read(&mut buf).unwrap();
            if len == 0 {
                break;
            }
            request.extend_from_slice(&buf[..len]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8_lossy(&request).into_owned()
    }

    fn test_dir(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "msr-file-fetcher-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}
