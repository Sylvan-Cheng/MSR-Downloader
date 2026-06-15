use crate::api::{FileProgress, MusicSource};
use crate::config::Config;
use crate::fs_util;
use crate::metadata;
use crate::models::{AlbumDetail, SongDetail};
use crate::progress::{
    AlbumDownloadReport, DownloadEvent, DownloadIssue, DownloadIssueKind, DownloadProgress,
    EventSink, MaybeEventSink, ProgressSnapshotAdapter, SongStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Internal context shared across download helpers.
///
/// Bundles the optional progress handle (for CLI rendering) and the optional
/// event sink (for Tauri / event-driven consumers) so that every helper does
/// not need to accept them as separate parameters.
#[derive(Clone)]
struct DownloadContext {
    progress: ProgressSnapshotAdapter,
    sink: MaybeEventSink,
    cancellation: DownloadCancellation,
}

#[derive(Clone, Debug, Default)]
pub struct DownloadCancellation {
    cancelled: Arc<AtomicBool>,
}

impl DownloadCancellation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDownloadOptions {
    pub track_selection: TrackSelection,
}

impl AlbumDownloadOptions {
    pub fn all_tracks() -> Self {
        Self::default()
    }

    pub fn track_indices(indices: Vec<usize>) -> Self {
        Self {
            track_selection: TrackSelection::Indices(indices),
        }
    }

    pub fn song_ids(song_ids: Vec<String>) -> Self {
        Self {
            track_selection: TrackSelection::SongIds(song_ids),
        }
    }

    pub fn selected_track_count(&self, album: &AlbumDetail) -> usize {
        self.track_selection.selected_count(album)
    }

    pub fn selects_all_tracks(&self) -> bool {
        self.track_selection.is_all()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum TrackSelection {
    #[default]
    All,
    /// 1-based track indices as shown to users.
    Indices(Vec<usize>),
    SongIds(Vec<String>),
}

impl TrackSelection {
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    pub fn selected_count(&self, album: &AlbumDetail) -> usize {
        match self {
            Self::All => album.songs.len(),
            Self::Indices(indices) => indices.iter().copied().collect::<HashSet<_>>().len(),
            Self::SongIds(song_ids) => song_ids.iter().collect::<HashSet<_>>().len(),
        }
    }

    pub fn parse_indices_spec(spec: &str) -> anyhow::Result<Vec<usize>> {
        let mut indices = Vec::new();
        for raw_part in spec.split(',') {
            let part = raw_part.trim();
            if part.is_empty() {
                anyhow::bail!("invalid track selection: empty item in {spec:?}");
            }

            if let Some((start, end)) = part.split_once('-') {
                let start = parse_track_index(start.trim(), spec)?;
                let end = parse_track_index(end.trim(), spec)?;
                if start > end {
                    anyhow::bail!(
                        "invalid track selection: range {part:?} counts backward; use 1,3,5-8"
                    );
                }
                indices.extend(start..=end);
            } else {
                indices.push(parse_track_index(part, spec)?);
            }
        }

        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            anyhow::bail!("track selection cannot be empty");
        }
        Ok(indices)
    }

    fn selected_album_songs<'a>(
        &self,
        album: &'a AlbumDetail,
    ) -> anyhow::Result<Vec<(usize, &'a crate::models::SongBrief)>> {
        match self {
            Self::All => Ok(album.songs.iter().enumerate().collect()),
            Self::Indices(indices) => select_album_songs_by_indices(album, indices),
            Self::SongIds(song_ids) => select_album_songs_by_ids(album, song_ids),
        }
    }
}

fn parse_track_index(value: &str, spec: &str) -> anyhow::Result<usize> {
    let index: usize = value
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid track selection {spec:?}; use 1,3,5-8"))?;
    if index == 0 {
        anyhow::bail!("invalid track selection {spec:?}; track numbers start at 1");
    }
    Ok(index)
}

fn select_album_songs_by_indices<'a>(
    album: &'a AlbumDetail,
    indices: &[usize],
) -> anyhow::Result<Vec<(usize, &'a crate::models::SongBrief)>> {
    if indices.is_empty() {
        anyhow::bail!("track selection cannot be empty");
    }

    let mut seen = HashSet::new();
    let mut wanted = HashSet::new();
    for &index in indices {
        if index == 0 || index > album.songs.len() {
            anyhow::bail!(
                "track index {index} is out of range for album {} with {} track(s)",
                album.name,
                album.songs.len()
            );
        }
        if seen.insert(index) {
            wanted.insert(index - 1);
        }
    }

    Ok(album
        .songs
        .iter()
        .enumerate()
        .filter(|(idx, _)| wanted.contains(idx))
        .collect())
}

fn select_album_songs_by_ids<'a>(
    album: &'a AlbumDetail,
    song_ids: &[String],
) -> anyhow::Result<Vec<(usize, &'a crate::models::SongBrief)>> {
    if song_ids.is_empty() {
        anyhow::bail!("track selection cannot be empty");
    }

    let wanted: HashSet<&str> = song_ids.iter().map(String::as_str).collect();
    let selected: Vec<_> = album
        .songs
        .iter()
        .enumerate()
        .filter(|(_, song)| wanted.contains(song.cid.as_str()))
        .collect();

    if selected.len() != wanted.len() {
        let album_ids: HashSet<&str> = album.songs.iter().map(|song| song.cid.as_str()).collect();
        let missing = song_ids
            .iter()
            .find(|song_id| !album_ids.contains(song_id.as_str()))
            .map(String::as_str)
            .unwrap_or("<unknown>");
        anyhow::bail!("song id {missing} is not part of album {}", album.name);
    }

    Ok(selected)
}

impl DownloadContext {
    fn new(progress: ProgressSnapshotAdapter, sink: MaybeEventSink) -> Self {
        Self::new_cancelable(progress, sink, DownloadCancellation::new())
    }

    fn new_cancelable(
        progress: ProgressSnapshotAdapter,
        sink: MaybeEventSink,
        cancellation: DownloadCancellation,
    ) -> Self {
        Self {
            progress,
            sink,
            cancellation,
        }
    }

    fn with_progress(progress: Option<Arc<Mutex<DownloadProgress>>>) -> Self {
        let progress = progress
            .map(ProgressSnapshotAdapter::from_progress_handle)
            .unwrap_or_default();
        Self::new(progress, MaybeEventSink::none())
    }

    fn emit(&self, event: DownloadEvent) {
        self.progress.emit(event.clone());
        self.sink.emit(event);
    }

    fn ensure_not_cancelled(&self) -> anyhow::Result<()> {
        if self.cancellation.is_cancelled() {
            anyhow::bail!("download cancelled");
        }
        Ok(())
    }
}

use tokio::sync::Semaphore;

struct SongDownloadJob {
    album_path: PathBuf,
    song: SongDetail,
    album: AlbumDetail,
    config: Config,
    current: usize,
    total: usize,
    cover_data: Option<Vec<u8>>,
    ctx: DownloadContext,
}

struct MetadataWrite<'a> {
    config: &'a Config,
    dest: &'a Path,
    song: &'a SongDetail,
    album: &'a AlbumDetail,
    cover_data: Option<&'a [u8]>,
    lyrics_text: Option<String>,
    downloaded: bool,
    ctx: &'a DownloadContext,
}

pub async fn download_album_with_progress<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<AlbumDownloadReport> {
    download_album_with_options_progress(
        api,
        album,
        config,
        AlbumDownloadOptions::all_tracks(),
        progress,
    )
    .await
}

pub async fn download_album_with_options_progress<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    options: AlbumDownloadOptions,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<AlbumDownloadReport> {
    let progress = progress.unwrap_or_else(|| {
        Arc::new(Mutex::new(DownloadProgress::new(
            &album.name,
            options.selected_track_count(album),
        )))
    });
    let ctx = DownloadContext::with_progress(Some(progress));
    download_album_inner(api, album, config, &options, &ctx).await
}

pub async fn download_album_with_events<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    sink: impl EventSink + 'static,
) -> anyhow::Result<AlbumDownloadReport> {
    download_album_with_options_events(api, album, config, AlbumDownloadOptions::all_tracks(), sink)
        .await
}

pub async fn download_album_with_options_events<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    options: AlbumDownloadOptions,
    sink: impl EventSink + 'static,
) -> anyhow::Result<AlbumDownloadReport> {
    let ctx = DownloadContext::new(
        ProgressSnapshotAdapter::new(),
        MaybeEventSink::new(Some(Arc::new(sink))),
    );
    download_album_inner(api, album, config, &options, &ctx).await
}

pub async fn download_album_with_options_events_cancelable<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    options: AlbumDownloadOptions,
    sink: impl EventSink + 'static,
    cancellation: DownloadCancellation,
) -> anyhow::Result<AlbumDownloadReport> {
    let ctx = DownloadContext::new_cancelable(
        ProgressSnapshotAdapter::new(),
        MaybeEventSink::new(Some(Arc::new(sink))),
        cancellation,
    );
    download_album_inner(api, album, config, &options, &ctx).await
}

async fn download_album_inner<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    options: &AlbumDownloadOptions,
    ctx: &DownloadContext,
) -> anyhow::Result<AlbumDownloadReport> {
    let selected_count = options.selected_track_count(album);
    ctx.emit(DownloadEvent::AlbumStarted {
        album_name: album.name.clone(),
        total_tracks: selected_count,
    });

    match download_album_body(api, album, config, options, ctx).await {
        Ok(report) => {
            ctx.emit(DownloadEvent::AlbumFinished {
                report: report.clone(),
            });
            Ok(report)
        }
        Err(e) => {
            ctx.emit(DownloadEvent::AlbumFailed {
                error: e.to_string(),
            });
            Err(e)
        }
    }
}

async fn download_album_body<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    options: &AlbumDownloadOptions,
    ctx: &DownloadContext,
) -> anyhow::Result<AlbumDownloadReport> {
    ctx.ensure_not_cancelled()?;
    let selected_album_songs = options.track_selection.selected_album_songs(album)?;
    let selected_count = selected_album_songs.len();

    ctx.ensure_not_cancelled()?;
    let album_path = create_album_path(config, album)?;

    if config.download.include.album_info {
        save_album_info(&album_path, album)?;
    }

    let cover_data = if config.download.include.covers {
        download_covers(api, &album_path, album, ctx).await?
    } else {
        None
    };

    let concurrency = config.download.concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let total = selected_count;

    let mut song_details = Vec::with_capacity(total);
    for (selected_idx, (_, song_brief)) in selected_album_songs.into_iter().enumerate() {
        ctx.ensure_not_cancelled()?;
        let progress_index = selected_idx + 1;
        let song_detail = api.get_song(&song_brief.cid).await?;
        ctx.emit(DownloadEvent::TrackQueued {
            index: progress_index,
            name: song_detail.name.clone(),
        });
        song_details.push((progress_index, song_detail));
    }

    fs_util::validate_song_destinations(config, &album_path, &song_details)?;

    let mut handles = Vec::new();

    for (progress_index, song_detail) in song_details {
        ctx.ensure_not_cancelled()?;
        let api_clone = api.clone();
        let album_path_clone = album_path.clone();
        let config_clone = config.clone();
        let cover_data_clone = cover_data.clone();
        let semaphore_clone = semaphore.clone();
        let album_clone = album.clone();
        let ctx_clone = ctx.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore_clone.acquire().await.unwrap();

            download_song_with_progress(
                &api_clone,
                SongDownloadJob {
                    album_path: album_path_clone,
                    song: song_detail,
                    album: album_clone,
                    config: config_clone,
                    current: progress_index,
                    total,
                    cover_data: cover_data_clone,
                    ctx: ctx_clone,
                },
            )
            .await
        });

        handles.push(handle);
    }

    for handle in handles {
        ctx.ensure_not_cancelled()?;
        match handle.await {
            Ok(result) => {
                if let Err(e) = result {
                    let message = format!("Download error: {}", e);
                    push_issue(
                        ctx,
                        DownloadIssue::new(DownloadIssueKind::Audio, "", message),
                    );
                }
            }
            Err(e) => {
                let message = format!("Task error: {}", e);
                push_issue(
                    ctx,
                    DownloadIssue::new(DownloadIssueKind::Task, "", message),
                );
            }
        }
    }

    Ok(report_from_progress(ctx, album))
}

fn report_from_progress(ctx: &DownloadContext, album: &AlbumDetail) -> AlbumDownloadReport {
    let progress = ctx.progress.snapshot();
    if progress.album_name.is_empty() {
        return AlbumDownloadReport {
            album_name: album.name.clone(),
            total_tracks: album.songs.len(),
            tracks: Vec::new(),
            issues: Vec::new(),
        };
    }
    AlbumDownloadReport::from_progress(&progress)
}

fn create_album_path(config: &Config, album: &AlbumDetail) -> anyhow::Result<PathBuf> {
    let folder_name = config
        .naming
        .album_folder
        .replace("{album_name}", &fs_util::sanitize(&album.name));
    let path = fs_util::safe_join_child(&config.download.output_dir, &folder_name)?;
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn update_progress(
    ctx: &DownloadContext,
    current_song: usize,
    song_name: &str,
    file_progress: FileProgress,
) {
    let mut speed_bps = 0.0;
    let snapshot = ctx.progress.snapshot();
    if let Some(task) = snapshot
        .tasks
        .iter()
        .find(|task| task.index == current_song)
    {
        if let Some(previous_update) = task.last_update {
            let elapsed = Instant::now().duration_since(previous_update).as_secs_f64();
            let bytes_since = file_progress
                .downloaded
                .saturating_sub(task.bytes_downloaded);
            if elapsed > 0.0 && bytes_since > 0 {
                let instant_speed = bytes_since as f64 / elapsed;
                speed_bps = if task.speed_bps > 0.0 {
                    task.speed_bps * 0.7 + instant_speed * 0.3
                } else {
                    instant_speed
                };
            }
        }
    }

    ctx.emit(DownloadEvent::TrackProgress {
        index: current_song,
        name: song_name.to_string(),
        downloaded: file_progress.downloaded,
        total: file_progress.total,
        speed_bps,
        resumed: file_progress.resumed,
    });
}

fn set_progress_status(
    ctx: &DownloadContext,
    current_song: usize,
    song_name: &str,
    status: SongStatus,
) {
    ctx.emit(DownloadEvent::TrackStatus {
        index: current_song,
        name: song_name.to_string(),
        status,
    });
}

fn finish_progress(
    ctx: &DownloadContext,
    current_song: usize,
    song_name: &str,
    skipped: bool,
    failed: bool,
) {
    let status = if failed {
        SongStatus::Failed
    } else if skipped {
        SongStatus::Skipped
    } else {
        SongStatus::Done
    };

    ctx.emit(DownloadEvent::TrackFinished {
        index: current_song,
        name: song_name.to_string(),
        status,
    });
}

fn push_issue(ctx: &DownloadContext, issue: DownloadIssue) {
    ctx.emit(DownloadEvent::Issue { issue });
}

fn save_album_info(path: &Path, album: &AlbumDetail) -> anyhow::Result<()> {
    let mut content = format!("name: {}\n", album.name);
    content.push_str(&format!("intro:\n{}\n", album.intro));
    content.push_str(&format!("belong: {}\n", album.belong));
    content.push_str("songs:\n");

    for song in &album.songs {
        let artists = if song.artists.is_empty() {
            "Unknown".to_string()
        } else {
            song.artists.join("、")
        };
        content.push_str(&format!("  {} - {}\n", song.name, artists));
    }

    std::fs::write(path.join("album_info.txt"), content)?;
    Ok(())
}

async fn download_covers<A: MusicSource>(
    api: &A,
    path: &Path,
    album: &AlbumDetail,
    ctx: &DownloadContext,
) -> anyhow::Result<Option<Vec<u8>>> {
    let album_name = fs_util::sanitize(&album.name);
    let mut cover_data: Option<Vec<u8>> = None;

    let ext = fs_util::ext_from_url(&album.cover_url);
    let dest = fs_util::safe_join_child(path, &format!("{}_Cover.{}", album_name, ext))?;
    download_optional_file(
        api,
        &album.cover_url,
        &dest,
        DownloadIssueKind::Cover,
        album.name.as_str(),
        "Failed to download cover",
        ctx,
    )
    .await;

    if dest.exists() {
        cover_data = Some(std::fs::read(&dest)?);
    }

    let ext = fs_util::ext_from_url(&album.cover_de_url);
    let dest = fs_util::safe_join_child(path, &format!("{}_CoverDe.{}", album_name, ext))?;
    download_optional_file(
        api,
        &album.cover_de_url,
        &dest,
        DownloadIssueKind::Cover,
        album.name.as_str(),
        "Failed to download alternate cover",
        ctx,
    )
    .await;

    Ok(cover_data)
}

/// Download a non-critical file (cover, lyrics). Errors are recorded as
/// issues rather than propagated — a missing cover should not abort the album.
async fn download_optional_file<A: MusicSource>(
    api: &A,
    url: &str,
    dest: &Path,
    kind: DownloadIssueKind,
    item: &str,
    context: &str,
    ctx: &DownloadContext,
) {
    if dest.exists() {
        return;
    }

    if let Err(e) = api.download_file(url, dest).await {
        push_issue(
            ctx,
            DownloadIssue::new(kind, item, format!("{}: {}", context, e)),
        );
    }
}

async fn download_song_with_progress<A: MusicSource>(
    api: &A,
    job: SongDownloadJob,
) -> anyhow::Result<()> {
    let SongDownloadJob {
        album_path,
        song,
        album,
        config,
        current,
        total,
        cover_data,
        ctx,
    } = job;

    let dest = fs_util::build_song_path(&config, &album_path, &song)?;
    let existing_converted_dest = fs_util::existing_converted_dest(&config, &dest, &song);
    ctx.ensure_not_cancelled()?;

    let downloaded = if let Some(final_dest) = existing_converted_dest.as_deref() {
        skip_existing_converted_file(final_dest, &song, current, &ctx);
        false
    } else {
        download_audio_file(api, &song, &dest, current, total, &ctx).await?
    };

    let lyrics_text = download_lyrics(api, &config, &album_path, &song, &ctx).await?;
    ctx.ensure_not_cancelled()?;

    let final_dest = match existing_converted_dest {
        Some(path) => path,
        None => convert_if_needed(&config, &dest, &song, &ctx)?,
    };
    ctx.ensure_not_cancelled()?;

    if downloaded {
        set_progress_status(&ctx, current, &song.name, SongStatus::Tagging);
    }

    write_metadata_if_needed(MetadataWrite {
        config: &config,
        dest: &final_dest,
        song: &song,
        album: &album,
        cover_data: cover_data.as_deref(),
        lyrics_text,
        downloaded,
        ctx: &ctx,
    })?;

    if downloaded {
        finish_progress(&ctx, current, &song.name, false, false);
    }

    Ok(())
}

fn skip_existing_converted_file(
    final_dest: &Path,
    song: &SongDetail,
    current: usize,
    ctx: &DownloadContext,
) {
    let size = final_dest
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(1);
    update_progress(
        ctx,
        current,
        &song.name,
        FileProgress {
            downloaded: size,
            total: size,
            resumed: false,
            resume_from: 0,
            attempt: 0,
        },
    );
    finish_progress(ctx, current, &song.name, true, false);
}

async fn download_audio_file<A: MusicSource>(
    api: &A,
    song: &SongDetail,
    dest: &Path,
    current: usize,
    _total: usize,
    ctx: &DownloadContext,
) -> anyhow::Result<bool> {
    set_progress_status(ctx, current, &song.name, SongStatus::Checking);
    ctx.ensure_not_cancelled()?;

    if dest.exists() {
        if existing_file_is_complete(api, song, dest).await? {
            let size = dest.metadata().map(|metadata| metadata.len()).unwrap_or(1);
            update_progress(
                ctx,
                current,
                &song.name,
                FileProgress {
                    downloaded: size,
                    total: size,
                    resumed: false,
                    resume_from: 0,
                    attempt: 0,
                },
            );
            finish_progress(ctx, current, &song.name, true, false);
            return Ok(false);
        }

        let _ = tokio::fs::remove_file(dest).await;
    }

    let song_name = song.name.clone();
    set_progress_status(ctx, current, &song_name, SongStatus::Getting);
    ctx.ensure_not_cancelled()?;

    let ctx_clone = ctx.clone();
    let mut on_progress = move |file_progress| {
        update_progress(&ctx_clone, current, &song_name, file_progress);
    };
    let cancel_ctx = ctx.clone();
    let should_cancel = move || cancel_ctx.cancellation.is_cancelled();
    let result = api
        .download_file_with_progress_cancelable(
            &song.source_url,
            dest,
            &mut on_progress,
            &should_cancel,
        )
        .await;

    match result {
        Ok(_) => Ok(true),
        Err(e) => {
            finish_progress(ctx, current, &song.name, false, true);
            Err(e)
        }
    }
}

async fn existing_file_is_complete<A: MusicSource>(
    api: &A,
    song: &SongDetail,
    dest: &Path,
) -> anyhow::Result<bool> {
    let local_size = dest.metadata()?.len();
    if local_size == 0 {
        return Ok(false);
    }

    match api.content_length(&song.source_url).await {
        Ok(Some(remote_size)) => Ok(local_size == remote_size),
        Ok(None) => Ok(false),
        Err(_) => Ok(false),
    }
}

async fn download_lyrics<A: MusicSource>(
    api: &A,
    config: &Config,
    path: &Path,
    song: &SongDetail,
    ctx: &DownloadContext,
) -> anyhow::Result<Option<String>> {
    if !config.download.include.lyrics {
        return Ok(None);
    }

    let lyric_url = match &song.lyric_url {
        Some(url) => url,
        None => return Ok(None),
    };

    let song_name = fs_util::sanitize(&song.name);
    let lyric_ext = fs_util::ext_from_url(lyric_url);
    let lyric_dest = fs_util::safe_join_child(path, &format!("{}.{}", song_name, lyric_ext))?;

    download_optional_file(
        api,
        lyric_url,
        &lyric_dest,
        DownloadIssueKind::Lyrics,
        song.name.as_str(),
        "Failed to download lyrics",
        ctx,
    )
    .await;

    if lyric_dest.exists() {
        match std::fs::read_to_string(&lyric_dest) {
            Ok(text) => Ok(Some(text)),
            Err(e) => {
                let message = format!("Could not read lyrics for {}: {}", song.name, e);
                push_issue(
                    ctx,
                    DownloadIssue::new(DownloadIssueKind::Lyrics, song.name.as_str(), message),
                );
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}

fn convert_if_needed(
    config: &Config,
    dest: &Path,
    song: &SongDetail,
    ctx: &DownloadContext,
) -> anyhow::Result<PathBuf> {
    if !config.download.convert.enabled || !config.download.convert.wav_to_flac || !dest.exists() {
        return Ok(dest.to_path_buf());
    }

    let ext = fs_util::ext_from_url(&song.source_url);
    if ext.to_lowercase() != "wav" {
        return Ok(dest.to_path_buf());
    }

    let flac_path = dest.with_extension("flac");
    if fs_util::existing_converted_dest(config, dest, song).is_some() {
        return Ok(flac_path);
    }
    if flac_path.exists() {
        let _ = std::fs::remove_file(&flac_path);
    }

    match metadata::convert_wav_to_flac(dest, &flac_path, config.download.convert.flac_compression)
    {
        Ok(_) => {
            if config.download.convert.delete_original {
                std::fs::remove_file(dest)?;
            }
            Ok(flac_path)
        }
        Err(e) => {
            let message = format!("Failed to convert {} to FLAC: {}", song.name, e);
            push_issue(
                ctx,
                DownloadIssue::new(DownloadIssueKind::Audio, song.name.as_str(), message),
            );
            Ok(dest.to_path_buf())
        }
    }
}

fn write_metadata_if_needed(args: MetadataWrite<'_>) -> anyhow::Result<()> {
    let MetadataWrite {
        config,
        dest,
        song,
        album,
        cover_data,
        lyrics_text,
        downloaded,
        ctx,
    } = args;

    if !config.download.include.metadata || (!downloaded && dest.exists()) {
        return Ok(());
    }

    let artist = if song.artists.is_empty() {
        "Unknown".to_string()
    } else {
        song.artists.join(", ")
    };

    let track = album
        .songs
        .iter()
        .position(|s| s.cid == song.cid)
        .unwrap_or(0) as u32
        + 1;

    if let Err(e) = metadata::write_metadata(
        dest,
        &song.name,
        &artist,
        &album.name,
        track,
        cover_data,
        lyrics_text.as_deref(),
    ) {
        let message = format!("Failed to write metadata for {}: {}", song.name, e);
        push_issue(
            ctx,
            DownloadIssue::new(DownloadIssueKind::Metadata, song.name.as_str(), message),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MusicSource;
    use crate::models::{AlbumBrief, SongBrief};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc;

    #[derive(Clone, Default)]
    struct MockSource {
        albums: Vec<AlbumBrief>,
        album_details: HashMap<String, AlbumDetail>,
        song_details: HashMap<String, SongDetail>,
        files: HashMap<String, Vec<u8>>,
        content_lengths: HashMap<String, Option<u64>>,
        fail_audio_urls: HashSet<String>,
        fail_file_urls: HashSet<String>,
        fail_detail_cids: HashSet<String>,
        audio_calls: Arc<Mutex<Vec<String>>>,
        detail_calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl MusicSource for MockSource {
        async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>> {
            Ok(self.albums.clone())
        }

        async fn get_album_detail(&self, cid: &str) -> anyhow::Result<AlbumDetail> {
            self.detail_calls.lock().unwrap().push(cid.to_string());
            if self.fail_detail_cids.contains(cid) {
                anyhow::bail!("detail failed for {cid}");
            }

            Ok(self
                .album_details
                .get(cid)
                .cloned()
                .unwrap_or_else(|| album_detail(cid, &format!("Album {cid}",))))
        }

        async fn get_song(&self, cid: &str) -> anyhow::Result<SongDetail> {
            Ok(self
                .song_details
                .get(cid)
                .cloned()
                .unwrap_or_else(|| song_detail(cid, &format!("Song {cid}"))))
        }

        async fn download_file(&self, url: &str, dest: &Path) -> anyhow::Result<()> {
            if self.fail_file_urls.contains(url) {
                anyhow::bail!("file failed for {url}");
            }
            std::fs::write(dest, self.files.get(url).cloned().unwrap_or_default())?;
            Ok(())
        }

        async fn content_length(&self, url: &str) -> anyhow::Result<Option<u64>> {
            Ok(self
                .content_lengths
                .get(url)
                .copied()
                .unwrap_or_else(|| self.files.get(url).map(|file| file.len() as u64)))
        }

        async fn download_file_with_progress(
            &self,
            url: &str,
            dest: &Path,
            on_progress: &mut (dyn FnMut(FileProgress) + Send),
        ) -> anyhow::Result<()> {
            self.audio_calls.lock().unwrap().push(url.to_string());
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
    async fn download_album_continues_after_single_track_failure() {
        let root = test_dir("track-failure");
        let mut source = MockSource::default();
        source.album_details.insert(
            "album".to_string(),
            album_with_songs("album", "Album", &["ok", "bad"]),
        );
        source.song_details.insert(
            "ok".to_string(),
            song_detail_with_url("ok", "Ok", "mock://ok.wav", None),
        );
        source.song_details.insert(
            "bad".to_string(),
            song_detail_with_url("bad", "Bad", "mock://bad.wav", None),
        );
        source
            .files
            .insert("mock://ok.wav".to_string(), b"ok".to_vec());
        source.fail_audio_urls.insert("mock://bad.wav".to_string());
        let mut config = test_config(root.clone());
        config.download.concurrency = 2;

        let report = download_album_with_progress(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.total_tracks, 2);
        assert_eq!(report.ok_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert!(report.has_track_failures());
        assert_eq!(
            std::fs::read(root.join("Album").join("Ok.wav")).unwrap(),
            b"ok"
        );
        assert!(!root.join("Album").join("Bad.wav").exists());
    }

    #[tokio::test]
    async fn download_album_with_track_indices_downloads_only_selected_tracks() {
        let root = test_dir("selected-tracks");
        let mut source = MockSource::default();
        source.album_details.insert(
            "album".to_string(),
            album_with_songs("album", "Album", &["one", "two", "three"]),
        );
        source.song_details.insert(
            "one".to_string(),
            song_detail_with_url("one", "One", "mock://one.wav", None),
        );
        source.song_details.insert(
            "two".to_string(),
            song_detail_with_url("two", "Two", "mock://two.wav", None),
        );
        source.song_details.insert(
            "three".to_string(),
            song_detail_with_url("three", "Three", "mock://three.wav", None),
        );
        source
            .files
            .insert("mock://one.wav".to_string(), b"one".to_vec());
        source
            .files
            .insert("mock://three.wav".to_string(), b"three".to_vec());
        let config = test_config(root.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();

        let report = download_album_with_options_events(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            AlbumDownloadOptions::track_indices(vec![1, 3]),
            tx,
        )
        .await
        .unwrap();
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let queued: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                DownloadEvent::TrackQueued { index, .. } => Some(*index),
                _ => None,
            })
            .collect();

        assert_eq!(report.total_tracks, 2);
        assert_eq!(report.ok_count(), 2);
        assert_eq!(
            report
                .tracks
                .iter()
                .map(|track| track.index)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(queued, vec![1, 2]);
        assert_eq!(
            *source.audio_calls.lock().unwrap(),
            vec!["mock://one.wav".to_string(), "mock://three.wav".to_string()]
        );
        assert!(root.join("Album").join("One.wav").exists());
        assert!(!root.join("Album").join("Two.wav").exists());
        assert!(root.join("Album").join("Three.wav").exists());
    }

    #[test]
    fn track_selection_parse_indices_spec_accepts_lists_and_ranges() {
        assert_eq!(
            TrackSelection::parse_indices_spec("1,3,5-7").unwrap(),
            vec![1, 3, 5, 6, 7]
        );
        assert_eq!(
            TrackSelection::parse_indices_spec("3,1,3").unwrap(),
            vec![1, 3]
        );
    }

    #[test]
    fn track_selection_parse_indices_spec_rejects_invalid_specs() {
        assert!(TrackSelection::parse_indices_spec("3-1").is_err());
        assert!(TrackSelection::parse_indices_spec("0").is_err());
        assert!(TrackSelection::parse_indices_spec("1,,2").is_err());
    }

    #[tokio::test]
    async fn download_album_rejects_out_of_range_track_selection() {
        let root = test_dir("selected-tracks-out-of-range");
        let source = source_for_single_song("mock://song.wav", b"audio", None);
        let config = test_config(root);

        let error = download_album_with_options_progress(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            AlbumDownloadOptions::track_indices(vec![2]),
            None,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("out of range"));
    }

    #[tokio::test]
    async fn download_album_skips_existing_complete_file() {
        let root = test_dir("existing-complete");
        let album_dir = root.join("Album");
        std::fs::create_dir_all(&album_dir).unwrap();
        std::fs::write(album_dir.join("Song.wav"), b"complete").unwrap();
        let source = source_for_single_song("mock://song.wav", b"complete", None);
        let config = test_config(root.clone());

        let report = download_album_with_progress(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.skipped_count(), 1);
        assert_eq!(
            std::fs::read(album_dir.join("Song.wav")).unwrap(),
            b"complete"
        );
        assert!(source.audio_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn download_album_redownloads_existing_damaged_file() {
        let root = test_dir("existing-damaged");
        let album_dir = root.join("Album");
        std::fs::create_dir_all(&album_dir).unwrap();
        std::fs::write(album_dir.join("Song.wav"), b"bad").unwrap();
        let source = source_for_single_song("mock://song.wav", b"complete", None);
        let config = test_config(root.clone());

        let report = download_album_with_progress(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.ok_count(), 1);
        assert_eq!(
            std::fs::read(album_dir.join("Song.wav")).unwrap(),
            b"complete"
        );
        assert_eq!(
            *source.audio_calls.lock().unwrap(),
            vec!["mock://song.wav".to_string()]
        );
    }

    #[tokio::test]
    async fn auxiliary_failures_do_not_fail_track() {
        let root = test_dir("auxiliary-failures");
        let mut source =
            source_for_single_song("mock://song.flac", b"flac", Some("mock://song.lrc"));
        let album = source.album_details.get_mut("album").unwrap();
        album.cover_url = "mock://cover.jpg".to_string();
        album.cover_de_url = "mock://cover-de.jpg".to_string();
        source.fail_file_urls.extend([
            "mock://cover.jpg".to_string(),
            "mock://cover-de.jpg".to_string(),
            "mock://song.lrc".to_string(),
        ]);
        let mut config = test_config(root.clone());
        config.download.include.covers = true;
        config.download.include.lyrics = true;
        config.download.include.metadata = true;

        let report = download_album_with_progress(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.ok_count(), 1);
        assert_eq!(report.failed_count(), 0);
        assert!(!report.has_track_failures());
        assert_eq!(report.issue_count(DownloadIssueKind::Cover), 2);
        assert_eq!(report.issue_count(DownloadIssueKind::Lyrics), 1);
        assert_eq!(report.issue_count(DownloadIssueKind::Metadata), 1);
        assert_eq!(
            std::fs::read(root.join("Album").join("Song.flac")).unwrap(),
            b"flac"
        );
    }

    #[tokio::test]
    async fn download_album_writes_flac_metadata_without_auxiliary_issue() {
        let root = test_dir("flac-metadata");
        let source = source_for_single_song("mock://song.flac", &minimal_flac_bytes(), None);
        let mut config = test_config(root.clone());
        config.download.include.metadata = true;

        let report = download_album_with_progress(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            None,
        )
        .await
        .unwrap();

        assert_eq!(report.ok_count(), 1);
        assert_eq!(report.issue_count(DownloadIssueKind::Metadata), 0);
        let tag = metaflac::Tag::read_from_path(root.join("Album").join("Song.flac")).unwrap();
        assert_eq!(
            tag.get_vorbis("TITLE").unwrap().collect::<Vec<_>>(),
            ["Song"]
        );
        assert_eq!(
            tag.get_vorbis("ALBUM").unwrap().collect::<Vec<_>>(),
            ["Album"]
        );
        assert_eq!(
            tag.get_vorbis("TRACKNUMBER").unwrap().collect::<Vec<_>>(),
            ["1"]
        );
    }

    #[tokio::test]
    async fn download_album_with_events_emits_expected_events() {
        let root = test_dir("events");
        let source = source_for_single_song("mock://song.wav", b"audio", None);
        let config = test_config(root.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();

        download_album_with_events(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            tx,
        )
        .await
        .unwrap();

        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events
            .iter()
            .any(|e| matches!(e, DownloadEvent::AlbumStarted { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, DownloadEvent::TrackQueued { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, DownloadEvent::TrackFinished { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, DownloadEvent::AlbumFinished { .. })));
    }

    #[tokio::test]
    async fn events_start_with_album_started_and_end_with_album_finished() {
        let root = test_dir("event-order");
        let source = source_for_single_song("mock://song.wav", b"audio", None);
        let config = test_config(root.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();

        download_album_with_events(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            tx,
        )
        .await
        .unwrap();

        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            matches!(events.first(), Some(DownloadEvent::AlbumStarted { .. })),
            "first event should be AlbumStarted, got {:?}",
            events.first()
        );
        assert!(
            matches!(events.last(), Some(DownloadEvent::AlbumFinished { .. })),
            "last event should be AlbumFinished, got {:?}",
            events.last()
        );
    }

    #[tokio::test]
    async fn preflight_failure_emits_album_failed() {
        let root = test_dir("preflight-fail");
        let mut source = MockSource::default();
        let mut album = album_with_songs("album", "Album", &["dup1", "dup2"]);
        album.songs[1].name = "Same".to_string();
        album.songs[0].name = "Same".to_string();
        source
            .album_details
            .insert("album".to_string(), album.clone());
        source
            .song_details
            .insert("dup1".to_string(), song_detail("dup1", "Same"));
        source
            .song_details
            .insert("dup2".to_string(), song_detail("dup2", "Same"));
        let config = test_config(root.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();

        let result = download_album_with_events(&source, &album, &config, tx).await;

        assert!(result.is_err());
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            matches!(events.first(), Some(DownloadEvent::AlbumStarted { .. })),
            "first event should be AlbumStarted"
        );
        assert!(
            matches!(events.last(), Some(DownloadEvent::AlbumFailed { .. })),
            "last event should be AlbumFailed, got {:?}",
            events.last()
        );
    }

    #[tokio::test]
    async fn track_failure_emits_track_finished_with_failed_status() {
        let root = test_dir("track-fail-event");
        let mut source = MockSource::default();
        source.album_details.insert(
            "album".to_string(),
            album_with_songs("album", "Album", &["bad"]),
        );
        source.song_details.insert(
            "bad".to_string(),
            song_detail_with_url("bad", "Bad", "mock://bad.wav", None),
        );
        source.fail_audio_urls.insert("mock://bad.wav".to_string());
        let config = test_config(root.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();

        let report = download_album_with_events(
            &source,
            source.album_details.get("album").unwrap(),
            &config,
            tx,
        )
        .await
        .unwrap();

        assert_eq!(report.failed_count(), 1);
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        let finished_events: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DownloadEvent::TrackFinished {
                        status: SongStatus::Failed,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(
            finished_events.len(),
            1,
            "exactly one TrackFinished(Failed)"
        );
        let album_finished = events
            .iter()
            .any(|e| matches!(e, DownloadEvent::AlbumFinished { .. }));
        assert!(album_finished, "AlbumFinished should still be emitted");
    }

    fn album_detail(cid: &str, name: &str) -> AlbumDetail {
        AlbumDetail {
            cid: cid.to_string(),
            name: name.to_string(),
            intro: String::new(),
            belong: String::new(),
            cover_url: String::new(),
            cover_de_url: String::new(),
            songs: Vec::new(),
        }
    }

    fn album_with_songs(cid: &str, name: &str, song_cids: &[&str]) -> AlbumDetail {
        let mut album = album_detail(cid, name);
        album.songs = song_cids
            .iter()
            .map(|cid| SongBrief {
                cid: (*cid).to_string(),
                name: if *cid == "ok" || *cid == "bad" {
                    cid[..1].to_uppercase() + &cid[1..]
                } else {
                    "Song".to_string()
                },
                artists: Vec::new(),
            })
            .collect();
        album
    }

    fn song_detail(cid: &str, name: &str) -> SongDetail {
        song_detail_with_url(cid, name, "https://example.com/song.wav", None)
    }

    fn song_detail_with_url(
        cid: &str,
        name: &str,
        source_url: &str,
        lyric_url: Option<&str>,
    ) -> SongDetail {
        SongDetail {
            cid: cid.to_string(),
            name: name.to_string(),
            album_cid: "album".to_string(),
            source_url: source_url.to_string(),
            lyric_url: lyric_url.map(str::to_string),
            mv_url: None,
            mv_cover_url: None,
            artists: Vec::new(),
        }
    }

    fn source_for_single_song(url: &str, body: &[u8], lyric_url: Option<&str>) -> MockSource {
        let mut source = MockSource::default();
        source.album_details.insert(
            "album".to_string(),
            album_with_songs("album", "Album", &["song"]),
        );
        source.song_details.insert(
            "song".to_string(),
            song_detail_with_url("song", "Song", url, lyric_url),
        );
        source.files.insert(url.to_string(), body.to_vec());
        source
    }

    fn minimal_flac_bytes() -> Vec<u8> {
        let mut bytes = b"fLaC".to_vec();
        bytes.push(0x80);
        bytes.extend_from_slice(&[0, 0, 34]);
        bytes.extend_from_slice(&[0; 34]);
        bytes
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
