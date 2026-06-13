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
}

impl DownloadProgress {
    pub fn new(album_name: &str, total_songs: usize) -> Self {
        Self {
            album_name: album_name.to_string(),
            total_songs,
            completed_songs: 0,
            tasks: Vec::new(),
            errors: Vec::new(),
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
            task.last_update = Some(Instant::now());
            return task;
        }

        self.tasks.push(SongProgress::new(index, name, status));
        self.tasks.last_mut().expect("task inserted")
    }
}
