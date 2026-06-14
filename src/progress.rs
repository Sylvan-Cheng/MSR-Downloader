use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    pub fn is_track_failure(self) -> bool {
        matches!(self, Self::Audio | Self::Task)
    }
}

#[derive(Clone, Debug)]
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

    pub(crate) fn active_for_plain_output(&self) -> bool {
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

#[derive(Clone, Debug)]
pub struct TrackDownloadReport {
    pub index: usize,
    pub name: String,
    pub status: SongStatus,
}

#[derive(Clone, Debug)]
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

    pub fn track_failure_count(&self) -> usize {
        self.failed_count()
            + self
                .issues
                .iter()
                .filter(|issue| issue.kind.is_track_failure())
                .count()
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

    pub(crate) fn task_mut_or_insert(
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
}
