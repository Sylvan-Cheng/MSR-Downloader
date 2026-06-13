use futures_util::StreamExt;
use reqwest::{
    header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE},
    Client, StatusCode,
};
use std::path::Path;
use tokio::io::AsyncWriteExt;

#[derive(Clone, Copy, Debug, Default)]
pub struct FileProgress {
    pub downloaded: u64,
    pub total: u64,
    pub resumed: bool,
    pub resume_from: u64,
    pub attempt: u32,
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

    loop {
        attempt += 1;
        let result =
            try_download_with_progress(client, url, dest, &temp_dest, attempt, &mut on_progress)
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
    attempt: u32,
    on_progress: &mut F,
) -> anyhow::Result<()>
where
    F: FnMut(FileProgress),
{
    let mut resume_from = tokio::fs::metadata(temp_dest)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    let requested_resume_from = resume_from;
    let mut request = client.get(url).header(ACCEPT_ENCODING, "identity");
    if resume_from > 0 {
        request = request.header(RANGE, format!("bytes={}-", resume_from));
    }

    let resp = request.send().await?;
    if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = tokio::fs::remove_file(temp_dest).await;
        anyhow::bail!("server rejected resume range");
    }

    let status = resp.status();
    let headers = resp.headers().clone();
    let resp = resp.error_for_status()?;

    if resume_from > 0 && status != StatusCode::PARTIAL_CONTENT {
        let _ = tokio::fs::remove_file(temp_dest).await;
        resume_from = 0;
    }

    let response_len = resp.content_length().unwrap_or(0);
    let total_size = if status == StatusCode::PARTIAL_CONTENT {
        parse_content_range_total(&headers).unwrap_or(resume_from + response_len)
    } else {
        response_len
    };

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

        if elapsed >= 500 || bytes_since >= 1024 * 1024 {
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
    on_progress(FileProgress {
        downloaded,
        total: total_size,
        resumed: resume_from > 0,
        resume_from: requested_resume_from,
        attempt,
    });
    Ok(())
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

fn temp_download_path(dest: &Path) -> std::path::PathBuf {
    let file_name = dest
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "download".to_string());
    dest.with_file_name(format!("{}.part", file_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

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
}
