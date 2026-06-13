use crate::api::{ApiClient, FileProgress};
use crate::config::Config;
use crate::metadata;
use crate::models::{AlbumDetail, SongDetail};
use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType},
};
use owo_colors::OwoColorize;
use std::collections::HashSet;
use std::io::{self, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Semaphore;

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

#[derive(Clone, Copy, Debug)]
pub enum CliProgressMode {
    Auto,
    Plain,
    Summary,
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
    pub done: bool,
    pub skipped: bool,
    pub failed: bool,
}

impl SongProgress {
    fn active_for_plain_output(&self) -> bool {
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

struct SongDownloadJob {
    album_path: PathBuf,
    song: SongDetail,
    album: AlbumDetail,
    config: Config,
    current: usize,
    total: usize,
    cover_data: Option<Vec<u8>>,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
}

pub async fn download_album(
    api: &ApiClient,
    album: &AlbumDetail,
    config: &Config,
    progress_mode: CliProgressMode,
) -> anyhow::Result<()> {
    let progress = Arc::new(Mutex::new(DownloadProgress::new(
        &album.name,
        album.songs.len(),
    )));
    let progress_clone = progress.clone();
    let download = tokio::spawn({
        let api = api.clone();
        let album = album.clone();
        let config = config.clone();
        async move { download_album_with_progress(&api, &album, &config, Some(progress_clone)).await }
    });

    render_cli_progress(&progress, &download, progress_mode).await?;
    download.await??;
    Ok(())
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
        self.tasks.iter().filter(|task| task.failed).count()
    }

    pub fn skipped_count(&self) -> usize {
        self.tasks.iter().filter(|task| task.skipped).count()
    }

    pub fn ok_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.done && !task.failed && !task.skipped)
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
}

pub async fn download_album_with_progress(
    api: &ApiClient,
    album: &AlbumDetail,
    config: &Config,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<()> {
    let album_path = create_album_path(config, album)?;

    if config.download.include.album_info {
        save_album_info(&album_path, album)?;
    }

    let cover_data = if config.download.include.covers {
        download_covers(api, &album_path, album).await?
    } else {
        None
    };

    let concurrency = config.download.concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let total = album.songs.len();

    let mut song_details = Vec::with_capacity(total);
    for (idx, song_brief) in album.songs.iter().enumerate() {
        let song_detail = api.get_song(&song_brief.cid).await?;
        set_progress_status(&progress, idx + 1, &song_detail.name, SongStatus::Queued);
        song_details.push((idx, song_detail));
    }

    validate_song_destinations(config, &album_path, &song_details)?;

    let mut handles = Vec::new();

    for (idx, song_detail) in song_details {
        let api_clone = api.clone();
        let album_path_clone = album_path.clone();
        let config_clone = config.clone();
        let cover_data_clone = cover_data.clone();
        let progress_clone = progress.clone();
        let semaphore_clone = semaphore.clone();
        let album_clone = album.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore_clone.acquire().await.unwrap();

            download_song_with_progress(
                &api_clone,
                SongDownloadJob {
                    album_path: album_path_clone,
                    song: song_detail,
                    album: album_clone,
                    config: config_clone,
                    current: idx + 1,
                    total,
                    cover_data: cover_data_clone,
                    progress: progress_clone,
                },
            )
            .await
        });

        handles.push(handle);
    }

    let mut failures = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(result) => {
                if let Err(e) = result {
                    let message = format!("Download error: {}", e);
                    push_error(&progress, message.clone());
                    failures.push(message);
                }
            }
            Err(e) => {
                let message = format!("Task error: {}", e);
                push_error(&progress, message.clone());
                failures.push(message);
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} track(s) failed: {}",
            failures.len(),
            failures.join("; ")
        );
    }

    Ok(())
}

fn create_album_path(config: &Config, album: &AlbumDetail) -> anyhow::Result<PathBuf> {
    let folder_name = config
        .naming
        .album_folder
        .replace("{album_name}", &sanitize(&album.name));
    let path = safe_join_child(&config.download.output_dir, &folder_name)?;
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn validate_song_destinations(
    config: &Config,
    album_path: &Path,
    songs: &[(usize, SongDetail)],
) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for (_, song) in songs {
        let path = build_song_path(config, album_path, song)?;
        if !seen.insert(path.clone()) {
            anyhow::bail!(
                "duplicate output path for song {}: {}",
                song.name,
                path.display()
            );
        }
    }
    Ok(())
}

fn update_progress(
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
    current_song: usize,
    song_name: &str,
    file_progress: FileProgress,
) {
    if let Some(ref p) = progress {
        if let Ok(mut prog) = p.lock() {
            let now = Instant::now();
            if let Some(task) = prog
                .tasks
                .iter_mut()
                .find(|task| task.index == current_song)
            {
                let previous_bytes = task.bytes_downloaded;
                let previous_update = task.last_update;
                task.name = song_name.to_string();
                task.bytes_downloaded = file_progress.downloaded;
                task.total_bytes = file_progress.total;
                task.resumed = file_progress.resumed;
                task.resume_from = file_progress.resume_from;
                task.attempt = file_progress.attempt;
                task.status = if file_progress.resumed {
                    SongStatus::Resuming
                } else {
                    SongStatus::Getting
                };
                if let Some(previous_update) = previous_update {
                    let elapsed = now.duration_since(previous_update).as_secs_f64();
                    let bytes_since = file_progress.downloaded.saturating_sub(previous_bytes);
                    if elapsed > 0.0 && bytes_since > 0 {
                        let instant_speed = bytes_since as f64 / elapsed;
                        task.speed_bps = if task.speed_bps > 0.0 {
                            task.speed_bps * 0.7 + instant_speed * 0.3
                        } else {
                            instant_speed
                        };
                    }
                }
                task.last_update = Some(now);
                return;
            }

            prog.tasks.push(SongProgress {
                index: current_song,
                name: song_name.to_string(),
                bytes_downloaded: file_progress.downloaded,
                total_bytes: file_progress.total,
                status: if file_progress.resumed {
                    SongStatus::Resuming
                } else {
                    SongStatus::Getting
                },
                resumed: file_progress.resumed,
                resume_from: file_progress.resume_from,
                attempt: file_progress.attempt,
                speed_bps: 0.0,
                last_update: Some(now),
                done: false,
                skipped: false,
                failed: false,
            });
        }
    }
}

fn set_progress_status(
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
    current_song: usize,
    song_name: &str,
    status: SongStatus,
) {
    if let Some(ref p) = progress {
        if let Ok(mut prog) = p.lock() {
            if let Some(task) = prog
                .tasks
                .iter_mut()
                .find(|task| task.index == current_song)
            {
                task.name = song_name.to_string();
                task.status = status;
                task.last_update = Some(Instant::now());
                return;
            }

            prog.tasks.push(SongProgress {
                index: current_song,
                name: song_name.to_string(),
                bytes_downloaded: 0,
                total_bytes: 0,
                status,
                resumed: false,
                resume_from: 0,
                attempt: 0,
                speed_bps: 0.0,
                last_update: Some(Instant::now()),
                done: false,
                skipped: false,
                failed: false,
            });
        }
    }
}

fn finish_progress(
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
    current_song: usize,
    skipped: bool,
    failed: bool,
) {
    if let Some(ref p) = progress {
        if let Ok(mut prog) = p.lock() {
            let mut counted = false;
            if let Some(task) = prog
                .tasks
                .iter_mut()
                .find(|task| task.index == current_song)
            {
                counted = !task.done;
                task.done = true;
                task.skipped = skipped;
                task.failed = failed;
                task.status = if failed {
                    SongStatus::Failed
                } else if skipped {
                    SongStatus::Skipped
                } else {
                    SongStatus::Done
                };
                task.speed_bps = 0.0;
                task.last_update = Some(Instant::now());
            }
            if counted {
                prog.completed_songs += 1;
            }
        }
    }
}

fn push_error(progress: &Option<Arc<Mutex<DownloadProgress>>>, message: String) {
    if let Some(ref p) = progress {
        if let Ok(mut prog) = p.lock() {
            prog.errors.push(message);
        }
    } else {
        eprintln!("  {} {}", "✗".red().bold(), message.red());
    }
}

fn save_album_info(path: &Path, album: &AlbumDetail) -> anyhow::Result<()> {
    let mut content = format!("name: {}\n", album.name);
    content.push_str(&format!("intro:\n{}\n", album.intro));
    content.push_str(&format!("belong: {}\n", album.belong));
    content.push_str("songs:\n");

    for song in &album.songs {
        let artists = if song.artistes.is_empty() {
            "Unknown".to_string()
        } else {
            song.artistes.join("、")
        };
        content.push_str(&format!("  {} - {}\n", song.name, artists));
    }

    std::fs::write(path.join("album_info.txt"), content)?;
    Ok(())
}

async fn download_covers(
    api: &ApiClient,
    path: &Path,
    album: &AlbumDetail,
) -> anyhow::Result<Option<Vec<u8>>> {
    let album_name = sanitize(&album.name);
    let mut cover_data: Option<Vec<u8>> = None;

    let ext = ext_from_url(&album.cover_url);
    let dest = safe_join_child(path, &format!("{}_Cover.{}", album_name, ext))?;
    if !dest.exists() {
        if let Err(e) = api.download_file(&album.cover_url, &dest).await {
            eprintln!(
                "  {} {}",
                "✗".red().bold(),
                format!("Failed to download cover: {}", e).red()
            );
        }
    }

    if dest.exists() {
        cover_data = Some(std::fs::read(&dest)?);
    }

    let ext = ext_from_url(&album.cover_de_url);
    let dest = safe_join_child(path, &format!("{}_CoverDe.{}", album_name, ext))?;
    if !dest.exists() {
        if let Err(e) = api.download_file(&album.cover_de_url, &dest).await {
            eprintln!(
                "  {} {}",
                "✗".red().bold(),
                format!("Failed to download cover: {}", e).red()
            );
        }
    }

    Ok(cover_data)
}

async fn download_song_with_progress(api: &ApiClient, job: SongDownloadJob) -> anyhow::Result<()> {
    let SongDownloadJob {
        album_path,
        song,
        album,
        config,
        current,
        total,
        cover_data,
        progress,
    } = job;

    let dest = build_song_path(&config, &album_path, &song)?;
    let existing_converted_dest = existing_converted_dest(&config, &dest, &song);

    let downloaded = if let Some(final_dest) = existing_converted_dest.as_deref() {
        skip_existing_converted_file(final_dest, &song, current, &progress);
        false
    } else {
        download_audio_file(api, &song, &dest, current, total, &progress).await?
    };

    let lyrics_text = download_lyrics(api, &config, &album_path, &song).await?;

    let final_dest = match existing_converted_dest {
        Some(path) => path,
        None => convert_if_needed(&config, &dest, &song)?,
    };

    if downloaded {
        set_progress_status(&progress, current, &song.name, SongStatus::Tagging);
    }

    write_metadata_if_needed(
        &config,
        &final_dest,
        &song,
        &album,
        cover_data.as_deref(),
        lyrics_text,
        downloaded,
    )?;

    if downloaded {
        finish_progress(&progress, current, false, false);
    }

    Ok(())
}

fn build_song_path(config: &Config, path: &Path, song: &SongDetail) -> anyhow::Result<PathBuf> {
    let song_name = sanitize(&song.name);
    let ext = ext_from_url(&song.source_url);

    let filename = config
        .naming
        .song_file
        .replace("{song_name}", &song_name)
        .replace("{ext}", &ext);

    safe_join_child(path, &filename)
}

fn existing_converted_dest(config: &Config, dest: &Path, song: &SongDetail) -> Option<PathBuf> {
    if !config.download.convert.enabled || !config.download.convert.wav_to_flac {
        return None;
    }

    if ext_from_url(&song.source_url).eq_ignore_ascii_case("wav") {
        let flac_path = dest.with_extension("flac");
        flac_path.exists().then_some(flac_path)
    } else {
        None
    }
}

fn skip_existing_converted_file(
    final_dest: &Path,
    song: &SongDetail,
    current: usize,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) {
    let size = final_dest
        .metadata()
        .map(|metadata| metadata.len())
        .unwrap_or(1);
    update_progress(
        progress,
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
    finish_progress(progress, current, true, false);
}

fn safe_join_child(base: &Path, child: &str) -> anyhow::Result<PathBuf> {
    if child.trim().is_empty() {
        anyhow::bail!("output path component cannot be empty");
    }

    let mut components = Path::new(child).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(base.join(child)),
        _ => anyhow::bail!("output path component must be a single file or folder name: {child}"),
    }
}

async fn download_audio_file(
    api: &ApiClient,
    song: &SongDetail,
    dest: &Path,
    current: usize,
    total: usize,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<bool> {
    set_progress_status(progress, current, &song.name, SongStatus::Checking);

    if dest.exists() {
        if existing_file_is_complete(api, song, dest).await? {
            let size = dest.metadata().map(|metadata| metadata.len()).unwrap_or(1);
            update_progress(
                progress,
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
            finish_progress(progress, current, true, false);
            return Ok(false);
        }

        let _ = tokio::fs::remove_file(dest).await;
    }

    let song_name = song.name.clone();
    set_progress_status(progress, current, &song_name, SongStatus::Getting);

    let result = api
        .download_file_with_progress(&song.source_url, dest, |file_progress| {
            update_progress(progress, current, &song_name, file_progress);
        })
        .await;

    match result {
        Ok(_) => Ok(true),
        Err(e) => {
            let _ = total;
            finish_progress(progress, current, false, true);
            Err(e)
        }
    }
}

async fn existing_file_is_complete(
    api: &ApiClient,
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
        Err(e) => {
            eprintln!(
                "  {} {}",
                "CHK".yellow().bold(),
                format!(
                    "Could not verify existing file for {}; re-downloading: {}",
                    song.name, e
                )
                .yellow()
            );
            Ok(false)
        }
    }
}

async fn render_cli_progress(
    progress: &Arc<Mutex<DownloadProgress>>,
    handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
    progress_mode: CliProgressMode,
) -> anyhow::Result<()> {
    if matches!(progress_mode, CliProgressMode::Summary) {
        return render_summary_only_cli_progress(progress, handle).await;
    }

    if matches!(progress_mode, CliProgressMode::Plain) || !io::stderr().is_terminal() {
        return render_plain_cli_progress(progress, handle).await;
    }

    let mut rendered_lines = 0usize;

    loop {
        let snapshot = progress.lock().ok().map(|progress| progress.clone());
        if let Some(snapshot) = snapshot {
            rendered_lines = draw_cli_progress(&snapshot, rendered_lines)?;
        }

        if handle.is_finished() {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }

    if let Ok(snapshot) = progress.lock().map(|progress| progress.clone()) {
        draw_cli_progress(&snapshot, rendered_lines)?;
        print_cli_summary(&snapshot);
    }
    eprintln!();
    Ok(())
}

async fn render_summary_only_cli_progress(
    progress: &Arc<Mutex<DownloadProgress>>,
    handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    while !handle.is_finished() {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    if let Ok(snapshot) = progress.lock().map(|progress| progress.clone()) {
        print_cli_summary(&snapshot);
    }
    Ok(())
}

async fn render_plain_cli_progress(
    progress: &Arc<Mutex<DownloadProgress>>,
    handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let mut last_completed = usize::MAX;
    let mut last_tick = Instant::now();

    loop {
        let snapshot = progress.lock().ok().map(|progress| progress.clone());
        if let Some(snapshot) = snapshot {
            let should_print = snapshot.completed_songs != last_completed
                || last_tick.elapsed() >= std::time::Duration::from_secs(2)
                || handle.is_finished();
            if should_print {
                print_plain_progress(&snapshot);
                last_completed = snapshot.completed_songs;
                last_tick = Instant::now();
            }
        }

        if handle.is_finished() {
            break;
        }

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    if let Ok(snapshot) = progress.lock().map(|progress| progress.clone()) {
        print_cli_summary(&snapshot);
    }
    Ok(())
}

fn draw_cli_progress(progress: &DownloadProgress, previous_lines: usize) -> anyhow::Result<usize> {
    let mut stderr = io::stderr();
    if previous_lines > 0 {
        execute!(
            stderr,
            cursor::MoveUp(previous_lines as u16),
            Clear(ClearType::FromCursorDown)
        )?;
    }

    let overall = if progress.total_songs > 0 {
        progress.completed_songs as f64 / progress.total_songs as f64
    } else {
        1.0
    };
    let mut lines = Vec::new();
    lines.push(format!(
        "{} {}  {}  {} ACTIVE  {}/s  ETA {}",
        "MSR//".cyan().bold(),
        progress.album_name.white().bold(),
        progress_line(
            overall,
            progress.completed_songs as u64,
            progress.total_songs as u64,
            "TRACKS"
        ),
        progress.active_count(),
        format_bytes(progress.total_speed_bps() as u64),
        progress
            .eta_seconds()
            .map(format_duration)
            .unwrap_or_else(|| "--:--".to_string())
    ));

    let mut tasks = progress.tasks.clone();
    tasks.sort_by_key(|task| task.index);
    for task in tasks.iter().rev().take(8).rev() {
        let ratio = if task.total_bytes > 0 {
            task.bytes_downloaded as f64 / task.total_bytes as f64
        } else {
            0.0
        };
        let status = colored_status(task.status);
        lines.push(format!(
            "  {} {:>2}/{:<2}  {}  {:>8}/s  {}",
            status,
            task.index,
            progress.total_songs,
            progress_line(ratio, task.bytes_downloaded, task.total_bytes, "MB"),
            format_bytes(task.speed_bps as u64),
            task.name
        ));
    }

    for error in progress.errors.iter().rev().take(3).rev() {
        lines.push(format!("  {} {}", "ERR".red().bold(), error.red()));
    }

    for line in &lines {
        eprintln!("{}", line);
    }
    stderr.flush()?;
    Ok(lines.len())
}

fn colored_status(status: SongStatus) -> String {
    match status {
        SongStatus::Failed => status.code().red().bold().to_string(),
        SongStatus::Skipped => status.code().dimmed().bold().to_string(),
        SongStatus::Done | SongStatus::Resuming => status.code().cyan().bold().to_string(),
        SongStatus::Checking | SongStatus::Tagging => status.code().yellow().bold().to_string(),
        SongStatus::Queued => status.code().dimmed().to_string(),
        SongStatus::Getting => status.code().white().bold().to_string(),
    }
}

fn print_plain_progress(progress: &DownloadProgress) {
    eprintln!(
        "MSR// {} TRACKS {}/{} ACTIVE {} SPEED {}/s ETA {}",
        progress.album_name,
        progress.completed_songs,
        progress.total_songs,
        progress.active_count(),
        format_bytes(progress.total_speed_bps() as u64),
        progress
            .eta_seconds()
            .map(format_duration)
            .unwrap_or_else(|| "--:--".to_string())
    );

    let mut tasks = progress.tasks.clone();
    tasks.sort_by_key(|task| task.index);
    let visible_tasks: Vec<_> = tasks
        .iter()
        .filter(|task| task.done || task.active_for_plain_output())
        .collect();
    let start = visible_tasks.len().saturating_sub(6);
    for task in &visible_tasks[start..] {
        let percent = (progress_ratio(task.bytes_downloaded, task.total_bytes) * 100.0).round();
        eprintln!(
            "{} {:>2}/{:<2} {:>3}% {}/{} {}/s {}",
            task.status.code(),
            task.index,
            progress.total_songs,
            percent as u64,
            format_bytes(task.bytes_downloaded),
            format_bytes(task.total_bytes),
            format_bytes(task.speed_bps as u64),
            task.name
        );
    }
}

fn print_cli_summary(progress: &DownloadProgress) {
    let status = if progress.failed_count() > 0 {
        "MSR// TRANSFER INCOMPLETE".red().bold().to_string()
    } else {
        "MSR// TRANSFER SUMMARY".cyan().bold().to_string()
    };
    eprintln!("\n{}", status);
    eprintln!(
        "  TRACKS  {} ok / {} skipped / {} failed",
        progress.ok_count(),
        progress.skipped_count(),
        progress.failed_count()
    );
    if progress.errors.is_empty() {
        return;
    }
    eprintln!("  FAILED");
    for error in progress.errors.iter().rev().take(5).rev() {
        eprintln!("  ERR  {}", error);
    }
}

fn progress_line(ratio: f64, current: u64, total: u64, unit: &str) -> String {
    let width = 28usize;
    let ratio = ratio.clamp(0.0, 1.0);
    let units = (ratio * width as f64).floor() as usize;
    let filled = if ratio >= 1.0 {
        width
    } else {
        units.min(width.saturating_sub(1))
    };
    let head = if ratio > 0.0 && ratio < 1.0 {
        "╸"
    } else {
        ""
    };
    let empty = width.saturating_sub(filled + head.chars().count());
    let bar = format!("{}{}{}", "─".repeat(filled), head, "·".repeat(empty));
    let percent = (ratio * 100.0).round() as u64;

    if unit == "MB" {
        let downloaded_mb = current as f64 / 1024.0 / 1024.0;
        let total_mb = total as f64 / 1024.0 / 1024.0;
        format!(
            "{} {:>3}% {:>6.1}/{:<6.1} MB",
            bar.cyan(),
            percent,
            downloaded_mb,
            total_mb
        )
    } else {
        format!(
            "{} {:>3}% {}/{} {}",
            bar.cyan(),
            percent,
            current,
            total,
            unit
        )
    }
}

pub fn progress_ratio(bytes_downloaded: u64, total_bytes: u64) -> f64 {
    if total_bytes > 0 {
        bytes_downloaded as f64 / total_bytes as f64
    } else {
        0.0
    }
}

pub fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1024.0 / 1024.0;
    format!("{:.1} MB", mb)
}

pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

async fn download_lyrics(
    api: &ApiClient,
    config: &Config,
    path: &Path,
    song: &SongDetail,
) -> anyhow::Result<Option<String>> {
    if !config.download.include.lyrics {
        return Ok(None);
    }

    let lyric_url = match &song.lyric_url {
        Some(url) => url,
        None => return Ok(None),
    };

    let song_name = sanitize(&song.name);
    let lyric_ext = ext_from_url(lyric_url);
    let lyric_dest = safe_join_child(path, &format!("{}.{}", song_name, lyric_ext))?;

    if !lyric_dest.exists() {
        if let Err(e) = api.download_file(lyric_url, &lyric_dest).await {
            eprintln!(
                "  {} {}",
                "✗".red().bold(),
                format!("Failed to download lyrics for {}: {}", song.name, e).red()
            );
            return Ok(None);
        }
    }

    if lyric_dest.exists() {
        match std::fs::read_to_string(&lyric_dest) {
            Ok(text) => Ok(Some(text)),
            Err(e) => {
                eprintln!(
                    "  {} {}",
                    "⚠".yellow().bold(),
                    format!("Could not read lyrics for {}: {}", song.name, e).yellow()
                );
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}

fn convert_if_needed(config: &Config, dest: &Path, song: &SongDetail) -> anyhow::Result<PathBuf> {
    if !config.download.convert.enabled || !config.download.convert.wav_to_flac || !dest.exists() {
        return Ok(dest.to_path_buf());
    }

    let ext = ext_from_url(&song.source_url);
    if ext.to_lowercase() != "wav" {
        return Ok(dest.to_path_buf());
    }

    let flac_path = dest.with_extension("flac");
    if flac_path.exists() {
        return Ok(flac_path);
    }

    match metadata::convert_wav_to_flac(dest, &flac_path, config.download.convert.flac_compression)
    {
        Ok(_) => {
            eprintln!(
                "  {} {}",
                "✓".green().bold(),
                format!("Converted to FLAC: {}", sanitize(&song.name)).green()
            );

            if config.download.convert.delete_original {
                std::fs::remove_file(dest)?;
            }
            Ok(flac_path)
        }
        Err(e) => {
            eprintln!(
                "  {} {}",
                "⚠".yellow().bold(),
                format!("Failed to convert to FLAC: {}", e).yellow()
            );
            Ok(dest.to_path_buf())
        }
    }
}

fn write_metadata_if_needed(
    config: &Config,
    dest: &Path,
    song: &SongDetail,
    album: &AlbumDetail,
    cover_data: Option<&[u8]>,
    lyrics_text: Option<String>,
    downloaded: bool,
) -> anyhow::Result<()> {
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
        eprintln!(
            "  {} {}",
            "⚠".yellow().bold(),
            format!("Failed to write metadata: {}", e).yellow()
        );
    }

    Ok(())
}

fn sanitize(name: &str) -> String {
    let illegal = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    let result: String = name
        .chars()
        .map(|c| if illegal.contains(&c) { ' ' } else { c })
        .collect();
    let sanitized = result.trim().trim_matches('.');
    if sanitized.is_empty() {
        "untitled".to_string()
    } else {
        sanitized.to_string()
    }
}

fn ext_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let filename = path.rsplit('/').next().unwrap_or(path);
    if filename.contains('.') {
        filename.rsplit('.').next().unwrap_or("bin").to_string()
    } else {
        "bin".to_string()
    }
}

pub async fn download_all(
    api: &ApiClient,
    config: &Config,
    progress_mode: CliProgressMode,
) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;
    println!("{} {} ALBUMS", "MSR//".cyan().bold(), albums.len());

    let mut failures = Vec::new();
    for (i, album_brief) in albums.iter().enumerate() {
        println!(
            "\n{} [{}/{}] {}",
            "ALBUM".cyan().bold(),
            i + 1,
            albums.len(),
            album_brief.name.white().bold()
        );
        match api.get_album_detail(&album_brief.cid).await {
            Ok(album_detail) => {
                if let Err(e) = download_album(api, &album_detail, config, progress_mode).await {
                    let message = format!("{}: {}", album_brief.name, e);
                    eprintln!("{} {}", "ERR".red().bold(), message.red());
                    failures.push(message);
                }
            }
            Err(e) => {
                let message = format!("{}: {}", album_brief.name, e);
                eprintln!("{} {}", "ERR".red().bold(), message.red());
                failures.push(message);
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} album(s) failed: {}",
            failures.len(),
            failures.join("; ")
        );
    }

    Ok(())
}

pub async fn download_albums_by_name(
    api: &ApiClient,
    config: &Config,
    names: &[String],
    progress_mode: CliProgressMode,
) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;
    let matched: Vec<_> = albums
        .iter()
        .filter(|a| {
            names
                .iter()
                .any(|n| a.name.to_lowercase().contains(&n.to_lowercase()))
        })
        .collect();

    if matched.is_empty() {
        anyhow::bail!("no albums matched the given names; use --list to inspect available albums");
    }

    println!(
        "{} {} MATCHING ALBUMS",
        "MSR//".cyan().bold(),
        matched.len()
    );

    let mut failures = Vec::new();
    for album_brief in matched {
        println!(
            "\n{} {}",
            "ALBUM".cyan().bold(),
            album_brief.name.white().bold()
        );
        match api.get_album_detail(&album_brief.cid).await {
            Ok(album_detail) => {
                if let Err(e) = download_album(api, &album_detail, config, progress_mode).await {
                    let message = format!("{}: {}", album_brief.name, e);
                    eprintln!("{} {}", "ERR".red().bold(), message.red());
                    failures.push(message);
                }
            }
            Err(e) => {
                let message = format!("{}: {}", album_brief.name, e);
                eprintln!("{} {}", "ERR".red().bold(), message.red());
                failures.push(message);
            }
        }
    }

    if !failures.is_empty() {
        anyhow::bail!(
            "{} album(s) failed: {}",
            failures.len(),
            failures.join("; ")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SongDetail;

    #[test]
    fn test_sanitize() {
        assert_eq!(sanitize("test:file?name"), "test file name");
        assert_eq!(sanitize("test*file|name"), "test file name");
        assert_eq!(sanitize("  test file  "), "test file");
        assert_eq!(sanitize("normal_file.mp3"), "normal_file.mp3");
        assert_eq!(sanitize("???"), "untitled");
    }

    #[test]
    fn test_ext_from_url() {
        assert_eq!(ext_from_url("https://example.com/file.mp3"), "mp3");
        assert_eq!(
            ext_from_url("https://example.com/file.wav?token=123"),
            "wav"
        );
        assert_eq!(ext_from_url("https://example.com/file.flac"), "flac");
        assert_eq!(ext_from_url("https://example.com/path/noext"), "bin");
    }

    #[test]
    fn safe_join_child_rejects_path_escape() {
        let base = Path::new("album");

        assert!(safe_join_child(base, "../song.mp3").is_err());
        assert!(safe_join_child(base, "nested/song.mp3").is_err());
        assert_eq!(
            safe_join_child(base, "song.mp3").unwrap(),
            base.join("song.mp3")
        );
    }

    #[test]
    fn validate_song_destinations_rejects_duplicates() {
        let config = Config::default();
        let songs = vec![(0, song_detail("1", "same")), (1, song_detail("2", "same"))];

        assert!(validate_song_destinations(&config, Path::new("album"), &songs).is_err());
    }

    #[test]
    fn existing_converted_dest_requires_enabled_existing_wav_conversion() {
        let root = std::env::temp_dir().join(format!(
            "msr-downloader-flac-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wav_path = root.join("song.wav");
        let flac_path = root.join("song.flac");
        std::fs::write(&flac_path, b"flac").unwrap();

        let mut config = Config::default();
        let song = song_detail("1", "song");
        assert!(existing_converted_dest(&config, &wav_path, &song).is_none());

        config.download.convert.enabled = true;
        config.download.convert.wav_to_flac = true;
        assert_eq!(
            existing_converted_dest(&config, &wav_path, &song),
            Some(flac_path)
        );

        let mp3_song = SongDetail {
            source_url: "https://example.com/song.mp3".to_string(),
            ..song_detail("2", "song")
        };
        assert!(existing_converted_dest(&config, &wav_path, &mp3_song).is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    fn song_detail(cid: &str, name: &str) -> SongDetail {
        SongDetail {
            cid: cid.to_string(),
            name: name.to_string(),
            album_cid: "album".to_string(),
            source_url: "https://example.com/song.wav".to_string(),
            lyric_url: None,
            mv_url: None,
            mv_cover_url: None,
            artists: Vec::new(),
        }
    }
}
