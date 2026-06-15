use crate::api::MusicSource;
use crate::config::Config;
use crate::downloader::{self, AlbumDownloadOptions, DownloadCancellation};
use crate::models::AlbumBrief;
use crate::progress::{
    AlbumDownloadReport, DownloadEvent, DownloadIssue, DownloadIssueKind, DownloadProgress,
    EventSink,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct AlbumDownloadRequest {
    pub album: AlbumBrief,
    pub options: AlbumDownloadOptions,
}

impl AlbumDownloadRequest {
    pub fn all_tracks(album: AlbumBrief) -> Self {
        Self {
            album,
            options: AlbumDownloadOptions::all_tracks(),
        }
    }

    pub fn new(album: AlbumBrief, options: AlbumDownloadOptions) -> Self {
        Self { album, options }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumDownloadFailure {
    pub album_cid: String,
    pub album_name: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct AlbumDownloadSessionReport {
    pub albums: Vec<AlbumDownloadReport>,
    pub failures: Vec<AlbumDownloadFailure>,
}

impl AlbumDownloadSessionReport {
    pub fn has_failures(&self) -> bool {
        !self.failures.is_empty()
    }

    pub fn failure_message(&self) -> String {
        self.failures
            .iter()
            .map(|failure| format!("{}: {}", failure.album_name, failure.message))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

pub async fn download_album_session_with_progress<A: MusicSource>(
    api: &A,
    config: &Config,
    requests: Vec<AlbumDownloadRequest>,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
) -> AlbumDownloadSessionReport {
    let mut session_report = AlbumDownloadSessionReport::default();

    for request in requests {
        let album_detail = match api.get_album_detail(&request.album.cid).await {
            Ok(album_detail) => album_detail,
            Err(error) => {
                let message = format!("Album {} detail error: {}", request.album.name, error);
                push_album_issue(&progress, &request.album.name, &message);
                session_report.failures.push(AlbumDownloadFailure {
                    album_cid: request.album.cid,
                    album_name: request.album.name,
                    message,
                });
                continue;
            }
        };

        if let Some(progress) = &progress {
            if let Ok(mut progress) = progress.lock() {
                *progress = DownloadProgress::new(
                    &album_detail.name,
                    request.options.selected_track_count(&album_detail),
                );
            }
        }

        match downloader::download_album_with_options_progress(
            api,
            &album_detail,
            config,
            request.options,
            progress.clone(),
        )
        .await
        {
            Ok(report) => session_report.albums.push(report),
            Err(error) => {
                let message = format!("Album {} download error: {}", request.album.name, error);
                push_album_issue(&progress, &request.album.name, &message);
                session_report.failures.push(AlbumDownloadFailure {
                    album_cid: request.album.cid,
                    album_name: request.album.name,
                    message,
                });
            }
        }
    }

    session_report
}

pub async fn download_album_session_with_events<A, S>(
    api: &A,
    config: &Config,
    requests: Vec<AlbumDownloadRequest>,
    sink: S,
) -> AlbumDownloadSessionReport
where
    A: MusicSource,
    S: EventSink + Clone + 'static,
{
    let mut session_report = AlbumDownloadSessionReport::default();

    for request in requests {
        let album_detail = match api.get_album_detail(&request.album.cid).await {
            Ok(album_detail) => album_detail,
            Err(error) => {
                let message = format!("Album {} detail error: {}", request.album.name, error);
                let issue = DownloadIssue::new(
                    DownloadIssueKind::Album,
                    request.album.name.as_str(),
                    message.clone(),
                );
                sink.emit(DownloadEvent::Issue { issue });
                sink.emit(DownloadEvent::AlbumFailed {
                    error: message.clone(),
                });
                session_report.failures.push(AlbumDownloadFailure {
                    album_cid: request.album.cid,
                    album_name: request.album.name,
                    message,
                });
                continue;
            }
        };

        match downloader::download_album_with_options_events(
            api,
            &album_detail,
            config,
            request.options,
            sink.clone(),
        )
        .await
        {
            Ok(report) => session_report.albums.push(report),
            Err(error) => {
                let message = format!("Album {} download error: {}", request.album.name, error);
                session_report.failures.push(AlbumDownloadFailure {
                    album_cid: request.album.cid,
                    album_name: request.album.name,
                    message,
                });
            }
        }
    }

    session_report
}

pub async fn download_album_session_with_events_cancelable<A, S>(
    api: &A,
    config: &Config,
    requests: Vec<AlbumDownloadRequest>,
    sink: S,
    cancellation: DownloadCancellation,
) -> AlbumDownloadSessionReport
where
    A: MusicSource,
    S: EventSink + Clone + 'static,
{
    let mut session_report = AlbumDownloadSessionReport::default();

    for request in requests {
        if cancellation.is_cancelled() {
            session_report.failures.push(AlbumDownloadFailure {
                album_cid: request.album.cid,
                album_name: request.album.name,
                message: "download cancelled".to_string(),
            });
            break;
        }

        let album_detail = match api.get_album_detail(&request.album.cid).await {
            Ok(album_detail) => album_detail,
            Err(error) => {
                let message = format!("Album {} detail error: {}", request.album.name, error);
                let issue = DownloadIssue::new(
                    DownloadIssueKind::Album,
                    request.album.name.as_str(),
                    message.clone(),
                );
                sink.emit(DownloadEvent::Issue { issue });
                sink.emit(DownloadEvent::AlbumFailed {
                    error: message.clone(),
                });
                session_report.failures.push(AlbumDownloadFailure {
                    album_cid: request.album.cid,
                    album_name: request.album.name,
                    message,
                });
                continue;
            }
        };

        match downloader::download_album_with_options_events_cancelable(
            api,
            &album_detail,
            config,
            request.options,
            sink.clone(),
            cancellation.clone(),
        )
        .await
        {
            Ok(report) => session_report.albums.push(report),
            Err(error) => {
                let message = format!("Album {} download error: {}", request.album.name, error);
                session_report.failures.push(AlbumDownloadFailure {
                    album_cid: request.album.cid,
                    album_name: request.album.name,
                    message,
                });
            }
        }
    }

    session_report
}

fn push_album_issue(
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
    album_name: &str,
    message: &str,
) {
    if let Some(progress) = progress {
        if let Ok(mut progress) = progress.lock() {
            progress.push_issue(DownloadIssue::new(
                DownloadIssueKind::Album,
                album_name,
                message,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::FileProgress;
    use crate::models::{AlbumDetail, SongBrief, SongDetail};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Clone, Default)]
    struct MockSource {
        album_details: HashMap<String, AlbumDetail>,
        song_details: HashMap<String, SongDetail>,
        files: HashMap<String, Vec<u8>>,
        fail_detail_cids: HashSet<String>,
        fail_audio_urls: HashSet<String>,
    }

    #[async_trait]
    impl MusicSource for MockSource {
        async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>> {
            Ok(Vec::new())
        }

        async fn get_album_detail(&self, cid: &str) -> anyhow::Result<AlbumDetail> {
            if self.fail_detail_cids.contains(cid) {
                anyhow::bail!("detail failed for {cid}");
            }
            self.album_details
                .get(cid)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing album {cid}"))
        }

        async fn get_song(&self, cid: &str) -> anyhow::Result<SongDetail> {
            self.song_details
                .get(cid)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing song {cid}"))
        }

        async fn download_file(&self, _url: &str, _dest: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        async fn content_length(&self, url: &str) -> anyhow::Result<Option<u64>> {
            Ok(self.files.get(url).map(|file| file.len() as u64))
        }

        async fn download_file_with_progress(
            &self,
            url: &str,
            dest: &Path,
            on_progress: &mut (dyn FnMut(FileProgress) + Send),
        ) -> anyhow::Result<()> {
            if self.fail_audio_urls.contains(url) {
                anyhow::bail!("audio failed for {url}");
            }
            let body = self.files.get(url).cloned().unwrap_or_default();
            std::fs::write(dest, &body)?;
            on_progress(FileProgress {
                downloaded: body.len() as u64,
                total: body.len() as u64,
                resumed: false,
                resume_from: 0,
                attempt: 1,
            });
            Ok(())
        }
    }

    #[tokio::test]
    async fn session_downloads_single_album_successfully() {
        let root = test_dir("session-single-success");
        let mut source = MockSource::default();
        source.add_album("album", "Album", &["song"]);
        source.add_song("song", "Song", "mock://song.wav", b"audio");
        let config = test_config(root.clone());

        let report = download_album_session_with_progress(
            &source,
            &config,
            vec![AlbumDownloadRequest::all_tracks(album_brief(
                "album", "Album",
            ))],
            None,
        )
        .await;

        assert!(!report.has_failures());
        assert_eq!(report.albums.len(), 1);
        assert_eq!(report.albums[0].album_name, "Album");
        assert_eq!(report.albums[0].ok_count(), 1);
        assert_eq!(
            std::fs::read(root.join("Album").join("Song.wav")).unwrap(),
            b"audio"
        );
    }

    #[tokio::test]
    async fn session_aggregates_partial_album_failures() {
        let root = test_dir("session-partial-failure");
        let mut source = MockSource::default();
        source.add_album("ok", "Ok Album", &["ok-song"]);
        source.add_song("ok-song", "Ok Song", "mock://ok.wav", b"ok");
        source.add_album("bad", "Bad Album", &["bad-song"]);
        source.add_song("bad-song", "Bad Song", "mock://bad.wav", b"bad");
        source.fail_audio_urls.insert("mock://bad.wav".to_string());
        let config = test_config(root.clone());

        let report = download_album_session_with_progress(
            &source,
            &config,
            vec![
                AlbumDownloadRequest::all_tracks(album_brief("ok", "Ok Album")),
                AlbumDownloadRequest::all_tracks(album_brief("bad", "Bad Album")),
            ],
            None,
        )
        .await;

        assert!(!report.has_failures());
        assert_eq!(report.albums.len(), 2);
        assert_eq!(report.albums[0].ok_count(), 1);
        assert_eq!(report.albums[1].failed_count(), 1);
        assert!(report.albums[1].has_track_failures());
    }

    #[tokio::test]
    async fn session_records_detail_failures_and_continues() {
        let root = test_dir("session-detail-failure");
        let mut source = MockSource::default();
        source.add_album("ok", "Ok Album", &["ok-song"]);
        source.add_song("ok-song", "Ok Song", "mock://ok.wav", b"ok");
        source.fail_detail_cids.insert("missing".to_string());
        let config = test_config(root);

        let report = download_album_session_with_progress(
            &source,
            &config,
            vec![
                AlbumDownloadRequest::all_tracks(album_brief("missing", "Missing Album")),
                AlbumDownloadRequest::all_tracks(album_brief("ok", "Ok Album")),
            ],
            None,
        )
        .await;

        assert!(report.has_failures());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].album_cid, "missing");
        assert!(report.failures[0].message.contains("detail error"));
        assert_eq!(report.albums.len(), 1);
        assert_eq!(report.albums[0].album_name, "Ok Album");
    }

    impl MockSource {
        fn add_album(&mut self, cid: &str, name: &str, song_cids: &[&str]) {
            self.album_details
                .insert(cid.to_string(), album_detail(cid, name, song_cids));
        }

        fn add_song(&mut self, cid: &str, name: &str, url: &str, body: &[u8]) {
            self.song_details
                .insert(cid.to_string(), song_detail(cid, name, url));
            self.files.insert(url.to_string(), body.to_vec());
        }
    }

    fn album_brief(cid: &str, name: &str) -> AlbumBrief {
        AlbumBrief {
            cid: cid.to_string(),
            name: name.to_string(),
            cover_url: String::new(),
            artists: Vec::new(),
        }
    }

    fn album_detail(cid: &str, name: &str, song_cids: &[&str]) -> AlbumDetail {
        AlbumDetail {
            cid: cid.to_string(),
            name: name.to_string(),
            intro: String::new(),
            belong: String::new(),
            cover_url: String::new(),
            cover_de_url: String::new(),
            songs: song_cids
                .iter()
                .map(|song_cid| SongBrief {
                    cid: (*song_cid).to_string(),
                    name: format!("Song {song_cid}"),
                    artists: Vec::new(),
                })
                .collect(),
        }
    }

    fn song_detail(cid: &str, name: &str, url: &str) -> SongDetail {
        SongDetail {
            cid: cid.to_string(),
            name: name.to_string(),
            album_cid: "album".to_string(),
            source_url: url.to_string(),
            lyric_url: None,
            mv_url: None,
            mv_cover_url: None,
            artists: Vec::new(),
        }
    }

    fn test_config(root: PathBuf) -> Config {
        let mut config = Config::default();
        config.download.output_dir = root;
        config.download.include.album_info = false;
        config.download.include.covers = false;
        config.download.include.lyrics = false;
        config.download.include.metadata = false;
        config.download.convert.enabled = false;
        config.download.convert.wav_to_flac = false;
        config
    }

    fn test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "msr-downloader-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
