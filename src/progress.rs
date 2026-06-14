use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

/// Trait for receiving download events.
///
/// Implementors decide how to deliver events (channel, callback, log, etc.).
/// The downloader calls [`EventSink::emit`] for every state change.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: DownloadEvent);
}

/// Wrapper that disables event delivery when no sink is configured.
/// Used internally so callers never need to match on `Option` to emit.
#[derive(Clone)]
pub(crate) struct MaybeEventSink(Option<Arc<dyn EventSink>>);

impl MaybeEventSink {
    pub(crate) fn none() -> Self {
        Self(None)
    }

    pub(crate) fn new(sink: Option<Arc<dyn EventSink>>) -> Self {
        Self(sink)
    }

    /// Emit an event. No-op if no sink is configured.
    /// Send failures are silently ignored — events are best-effort;
    /// the receiving end disconnecting (e.g. a closed Tauri window) must
    /// not abort the download.
    pub(crate) fn emit(&self, event: DownloadEvent) {
        if let Some(ref sink) = self.0 {
            sink.emit(event);
        }
    }
}

/// Blanket impl: any `Fn(DownloadEvent) + Send + Sync` is an `EventSink`.
impl<F: Fn(DownloadEvent) + Send + Sync> EventSink for F {
    fn emit(&self, event: DownloadEvent) {
        (self)(event);
    }
}

/// `EventSink` backed by a Tokio unbounded channel.
/// Kept for backward-compatibility with tests and internal callers.
impl EventSink for tokio::sync::mpsc::UnboundedSender<DownloadEvent> {
    fn emit(&self, event: DownloadEvent) {
        let _ = self.send(event);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SongStatus {
    Queued,
    Checking,
    Getting,
    Resuming,
    Tagging,
    Skipped,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadIssueKind {
    Audio,
    Cover,
    Lyrics,
    Metadata,
    Album,
    Task,
}

impl DownloadIssueKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Cover => "cover",
            Self::Lyrics => "lyrics",
            Self::Metadata => "metadata",
            Self::Album => "album",
            Self::Task => "task",
        }
    }

    /// Whether this issue kind belongs to track-level failure reporting.
    ///
    /// `Audio` remains track-related so summaries can separate it from
    /// auxiliary issues. `AlbumDownloadReport::track_failure_count` deliberately
    /// does not add audio issues on top of failed tracks, because audio errors
    /// already mark the corresponding track as `SongStatus::Failed`.
    pub fn is_track_failure(self) -> bool {
        matches!(self, Self::Audio | Self::Task)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadIssue {
    pub kind: DownloadIssueKind,
    pub item: String,
    pub message: String,
}

impl DownloadIssue {
    pub fn new(
        kind: DownloadIssueKind,
        item: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            item: item.into(),
            message: message.into(),
        }
    }

    pub fn summary(&self) -> String {
        if self.item.is_empty() {
            format!("{}: {}", self.kind.label(), self.message)
        } else {
            format!("{} {}: {}", self.kind.label(), self.item, self.message)
        }
    }
}

impl SongStatus {
    pub fn code(self) -> &'static str {
        match self {
            Self::Queued => "QUE",
            Self::Checking => "CHK",
            Self::Getting => "GET",
            Self::Resuming => "RES",
            Self::Tagging => "TAG",
            Self::Skipped => "SKP",
            Self::Done => "OK ",
            Self::Failed => "ERR",
        }
    }
}

#[derive(Clone)]
pub struct SongProgress {
    pub index: usize,
    pub name: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub status: SongStatus,
    pub resumed: bool,
    pub resume_from: u64,
    pub attempt: u32,
    pub speed_bps: f64,
    pub last_update: Option<Instant>,
}

impl SongProgress {
    pub(crate) fn new(index: usize, name: &str, status: SongStatus) -> Self {
        Self {
            index,
            name: name.to_string(),
            bytes_downloaded: 0,
            total_bytes: 0,
            status,
            resumed: false,
            resume_from: 0,
            attempt: 0,
            speed_bps: 0.0,
            last_update: Some(Instant::now()),
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(
            self.status,
            SongStatus::Done | SongStatus::Skipped | SongStatus::Failed
        )
    }

    pub fn is_skipped(&self) -> bool {
        self.status == SongStatus::Skipped
    }

    pub fn is_failed(&self) -> bool {
        self.status == SongStatus::Failed
    }

    pub fn active_for_plain_output(&self) -> bool {
        matches!(
            self.status,
            SongStatus::Checking | SongStatus::Getting | SongStatus::Resuming | SongStatus::Tagging
        )
    }
}

#[derive(Clone)]
pub struct DownloadProgress {
    pub album_name: String,
    pub total_songs: usize,
    pub completed_songs: usize,
    pub tasks: Vec<SongProgress>,
    pub errors: Vec<String>,
    pub issues: Vec<DownloadIssue>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDownloadReport {
    pub index: usize,
    pub name: String,
    pub status: SongStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDownloadReport {
    pub album_name: String,
    pub total_tracks: usize,
    pub tracks: Vec<TrackDownloadReport>,
    pub issues: Vec<DownloadIssue>,
}

impl AlbumDownloadReport {
    pub fn from_progress(progress: &DownloadProgress) -> Self {
        let mut tracks: Vec<_> = progress
            .tasks
            .iter()
            .map(|task| TrackDownloadReport {
                index: task.index,
                name: task.name.clone(),
                status: task.status,
            })
            .collect();
        tracks.sort_by_key(|track| track.index);

        Self {
            album_name: progress.album_name.clone(),
            total_tracks: progress.total_songs,
            tracks,
            issues: progress.issues.clone(),
        }
    }

    pub fn failed_count(&self) -> usize {
        self.tracks
            .iter()
            .filter(|track| track.status == SongStatus::Failed)
            .count()
    }

    pub fn skipped_count(&self) -> usize {
        self.tracks
            .iter()
            .filter(|track| track.status == SongStatus::Skipped)
            .count()
    }

    pub fn ok_count(&self) -> usize {
        self.tracks
            .iter()
            .filter(|track| track.status == SongStatus::Done)
            .count()
    }

    /// Total number of track-level failures.
    ///
    /// Counts tracks with `SongStatus::Failed` plus `Task` issues (which have
    /// no corresponding track entry). `Audio` issues are **not** added because
    /// the downloader already marks the track as `Failed` on audio errors —
    /// adding them here would double-count.
    pub fn track_failure_count(&self) -> usize {
        let task_issues = self
            .issues
            .iter()
            .filter(|issue| issue.kind == DownloadIssueKind::Task)
            .count();
        self.failed_count() + task_issues
    }

    pub fn has_track_failures(&self) -> bool {
        self.track_failure_count() > 0
    }

    pub fn issue_count(&self, kind: DownloadIssueKind) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.kind == kind)
            .count()
    }

    pub fn auxiliary_issue_count(&self) -> usize {
        self.issues
            .iter()
            .filter(|issue| !issue.kind.is_track_failure())
            .count()
    }
}

impl DownloadProgress {
    pub fn new(album_name: &str, total_songs: usize) -> Self {
        Self {
            album_name: album_name.to_string(),
            total_songs,
            completed_songs: 0,
            tasks: Vec::new(),
            errors: Vec::new(),
            issues: Vec::new(),
        }
    }

    pub fn failed_count(&self) -> usize {
        self.tasks.iter().filter(|task| task.is_failed()).count()
    }

    pub fn skipped_count(&self) -> usize {
        self.tasks.iter().filter(|task| task.is_skipped()).count()
    }

    pub fn ok_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == SongStatus::Done)
            .count()
    }

    pub fn active_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| {
                matches!(
                    task.status,
                    SongStatus::Checking
                        | SongStatus::Getting
                        | SongStatus::Resuming
                        | SongStatus::Tagging
                )
            })
            .count()
    }

    pub fn push_issue(&mut self, issue: DownloadIssue) {
        self.errors.push(issue.summary());
        self.issues.push(issue);
    }

    pub fn total_speed_bps(&self) -> f64 {
        self.tasks.iter().map(|task| task.speed_bps).sum()
    }

    pub fn eta_seconds(&self) -> Option<u64> {
        let remaining: u64 = self
            .tasks
            .iter()
            .filter(|task| task.total_bytes > task.bytes_downloaded)
            .map(|task| task.total_bytes - task.bytes_downloaded)
            .sum();
        let speed = self.total_speed_bps();
        if remaining > 0 && speed > 0.0 {
            Some((remaining as f64 / speed).ceil() as u64)
        } else {
            None
        }
    }

    pub fn task_mut_or_insert(
        &mut self,
        index: usize,
        name: &str,
        status: SongStatus,
    ) -> &mut SongProgress {
        if let Some(position) = self.tasks.iter().position(|task| task.index == index) {
            let task = &mut self.tasks[position];
            task.name = name.to_string();
            task.status = status;
            return task;
        }

        self.tasks.push(SongProgress::new(index, name, status));
        self.tasks.last_mut().expect("task inserted")
    }
}

/// Events emitted during a download.
///
/// Ordering guarantees (when delivered through [`EventSink`]):
/// - The first event is always `AlbumStarted`.
/// - The last event is always `AlbumFinished` or `AlbumFailed` (never both).
/// - `TrackQueued` events arrive before the corresponding `TrackProgress`.
/// - `TrackFinished` arrives exactly once for each track task that reaches a
///   terminal state. Album-level failures during preflight can end the stream
///   before queued tracks start downloading.
/// - `Issue` events may be interleaved at any point.
///
/// All events use `#[serde(rename_all = "camelCase")]` for JS-friendly JSON.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DownloadEvent {
    #[serde(rename_all = "camelCase")]
    AlbumStarted {
        album_name: String,
        total_tracks: usize,
    },
    #[serde(rename_all = "camelCase")]
    TrackQueued { index: usize, name: String },
    #[serde(rename_all = "camelCase")]
    TrackProgress {
        index: usize,
        name: String,
        downloaded: u64,
        total: u64,
        speed_bps: f64,
        resumed: bool,
    },
    #[serde(rename_all = "camelCase")]
    TrackFinished {
        index: usize,
        name: String,
        status: SongStatus,
    },
    #[serde(rename_all = "camelCase")]
    Issue { issue: DownloadIssue },
    #[serde(rename_all = "camelCase")]
    AlbumFinished { report: AlbumDownloadReport },
    #[serde(rename_all = "camelCase")]
    AlbumFailed { error: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn task_mut_or_insert_preserves_last_update_for_existing_task() {
        let mut progress = DownloadProgress::new("album", 1);
        let original_update = Instant::now() - Duration::from_secs(1);
        let task = progress.task_mut_or_insert(1, "song", SongStatus::Queued);
        task.last_update = Some(original_update);

        let task = progress.task_mut_or_insert(1, "song", SongStatus::Getting);

        assert_eq!(task.last_update, Some(original_update));
        assert_eq!(task.status, SongStatus::Getting);
    }

    #[test]
    fn download_event_json_uses_tagged_enum_and_camel_case() {
        let event = DownloadEvent::AlbumStarted {
            album_name: "Test".to_string(),
            total_tracks: 5,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"albumStarted""#), "got: {json}");
        assert!(json.contains(r#""albumName":"Test""#), "got: {json}");
        assert!(json.contains(r#""totalTracks":5"#), "got: {json}");
    }

    #[test]
    fn download_event_track_progress_json_shape() {
        let event = DownloadEvent::TrackProgress {
            index: 1,
            name: "Song".to_string(),
            downloaded: 1024,
            total: 4096,
            speed_bps: 512.0,
            resumed: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"trackProgress""#), "got: {json}");
        assert!(json.contains(r#""index":1"#), "got: {json}");
        assert!(json.contains(r#""speedBps":512.0"#), "got: {json}");
    }

    #[test]
    fn download_event_album_failed_json_shape() {
        let event = DownloadEvent::AlbumFailed {
            error: "network error".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "albumFailed");
        assert_eq!(json["error"], "network error");
    }

    #[test]
    fn song_status_json_is_camel_case() {
        let json = serde_json::to_value(SongStatus::Getting).unwrap();
        assert_eq!(json, "getting");
        let json = serde_json::to_value(SongStatus::Failed).unwrap();
        assert_eq!(json, "failed");
    }

    #[test]
    fn track_failure_count_does_not_double_count_audio_issues() {
        let report = AlbumDownloadReport {
            album_name: "test".to_string(),
            total_tracks: 1,
            tracks: vec![TrackDownloadReport {
                index: 1,
                name: "song".to_string(),
                status: SongStatus::Failed,
            }],
            issues: vec![DownloadIssue::new(
                DownloadIssueKind::Audio,
                "song",
                "download failed",
            )],
        };

        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.track_failure_count(), 1);
        assert!(report.has_track_failures());
    }

    #[test]
    fn track_failure_count_includes_task_issues_without_track() {
        let report = AlbumDownloadReport {
            album_name: "test".to_string(),
            total_tracks: 2,
            tracks: vec![TrackDownloadReport {
                index: 1,
                name: "ok".to_string(),
                status: SongStatus::Done,
            }],
            issues: vec![DownloadIssue::new(
                DownloadIssueKind::Task,
                "",
                "task panicked",
            )],
        };

        assert_eq!(report.failed_count(), 0);
        assert_eq!(report.track_failure_count(), 1);
        assert!(report.has_track_failures());
    }
}
