use crate::cli_style;
use crate::format;
use crate::progress::{DownloadProgress, SongStatus};
use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType},
};
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub enum CliProgressMode {
    Auto,
    Plain,
    Summary,
}

#[derive(Debug)]
pub struct CliInterrupted;

impl fmt::Display for CliInterrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "interrupted by Ctrl+C; partial .part files were kept for resume"
        )
    }
}

impl std::error::Error for CliInterrupted {}

pub(crate) fn is_interrupted(error: &anyhow::Error) -> bool {
    error.downcast_ref::<CliInterrupted>().is_some()
}

pub(crate) async fn render_cli_progress(
    progress: &Arc<Mutex<DownloadProgress>>,
    handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
    progress_mode: CliProgressMode,
) -> anyhow::Result<()> {
    if matches!(progress_mode, CliProgressMode::Summary) {
        return render_summary_only_cli_progress(progress, handle).await;
    }

    if matches!(progress_mode, CliProgressMode::Plain)
        || !io::stderr().is_terminal()
        || cli_style::no_color()
    {
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

        if let Err(error) = sleep_or_interrupt(std::time::Duration::from_millis(120)).await {
            handle.abort();
            return Err(error);
        }
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
        if let Err(error) = sleep_or_interrupt(std::time::Duration::from_millis(250)).await {
            handle.abort();
            return Err(error);
        }
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

        if let Err(error) = sleep_or_interrupt(std::time::Duration::from_millis(250)).await {
            handle.abort();
            return Err(error);
        }
    }

    if let Ok(snapshot) = progress.lock().map(|progress| progress.clone()) {
        print_cli_summary(&snapshot);
    }
    Ok(())
}

async fn sleep_or_interrupt(duration: std::time::Duration) -> anyhow::Result<()> {
    tokio::select! {
        _ = tokio::time::sleep(duration) => Ok(()),
        result = tokio::signal::ctrl_c() => {
            result?;
            Err(CliInterrupted.into())
        }
    }
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
        cli_style::msr(),
        cli_style::value(&progress.album_name),
        progress_line(
            overall,
            progress.completed_songs as u64,
            progress.total_songs as u64,
            "TRACKS"
        ),
        progress.active_count(),
        format::format_bytes(progress.total_speed_bps() as u64),
        progress
            .eta_seconds()
            .map(format::format_duration)
            .unwrap_or_else(|| "--:--".to_string())
    ));

    let mut tasks = progress.tasks.clone();
    tasks.sort_by_key(|task| task.index);
    for task in tasks.iter().rev().take(8).rev() {
        let ratio = format::progress_ratio(task.bytes_downloaded, task.total_bytes);
        let status = colored_status(task.status);
        lines.push(format!(
            "  {} {:>2}/{:<2}  {}  {:>8}/s  {}",
            status,
            task.index,
            progress.total_songs,
            progress_line(ratio, task.bytes_downloaded, task.total_bytes, "MB"),
            format::format_bytes(task.speed_bps as u64),
            task.name
        ));
    }

    for error in progress.errors.iter().rev().take(3).rev() {
        lines.push(format!(
            "  {} {}",
            cli_style::error("ERR"),
            cli_style::error(error)
        ));
    }

    for line in &lines {
        eprintln!("{}", line);
    }
    stderr.flush()?;
    Ok(lines.len())
}

fn colored_status(status: SongStatus) -> String {
    match status {
        SongStatus::Failed => cli_style::error(status.code()),
        SongStatus::Skipped => cli_style::dimmed(status.code()),
        SongStatus::Done | SongStatus::Resuming => cli_style::title(status.code()),
        SongStatus::Checking | SongStatus::Tagging => cli_style::warning(status.code()),
        SongStatus::Queued => cli_style::dimmed(status.code()),
        SongStatus::Getting => cli_style::value(status.code()),
    }
}

fn print_plain_progress(progress: &DownloadProgress) {
    eprintln!(
        "MSR// {} TRACKS {}/{} ACTIVE {} SPEED {}/s ETA {}",
        progress.album_name,
        progress.completed_songs,
        progress.total_songs,
        progress.active_count(),
        format::format_bytes(progress.total_speed_bps() as u64),
        progress
            .eta_seconds()
            .map(format::format_duration)
            .unwrap_or_else(|| "--:--".to_string())
    );

    let mut tasks = progress.tasks.clone();
    tasks.sort_by_key(|task| task.index);
    let visible_tasks: Vec<_> = tasks
        .iter()
        .filter(|task| task.is_done() || task.active_for_plain_output())
        .collect();
    let start = visible_tasks.len().saturating_sub(6);
    for task in &visible_tasks[start..] {
        let percent =
            (format::progress_ratio(task.bytes_downloaded, task.total_bytes) * 100.0).round();
        eprintln!(
            "{} {:>2}/{:<2} {:>3}% {}/{} {}/s {}",
            task.status.code(),
            task.index,
            progress.total_songs,
            percent as u64,
            format::format_bytes(task.bytes_downloaded),
            format::format_bytes(task.total_bytes),
            format::format_bytes(task.speed_bps as u64),
            task.name
        );
    }
}

fn print_cli_summary(progress: &DownloadProgress) {
    let has_issues = progress.failed_count() > 0 || !progress.errors.is_empty();
    let status = if has_issues {
        cli_style::error("MSR// TRANSFER INCOMPLETE")
    } else {
        cli_style::title("MSR// TRANSFER SUMMARY")
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
    let bar = format::progress_bar(ratio, width);
    let percent = (ratio * 100.0).round() as u64;

    if unit == "MB" {
        let downloaded_mb = current as f64 / 1024.0 / 1024.0;
        let total_mb = total as f64 / 1024.0 / 1024.0;
        format!(
            "{} {:>3}% {:>6.1}/{:<6.1} MB",
            cli_style::title(&bar),
            percent,
            downloaded_mb,
            total_mb
        )
    } else {
        format!(
            "{} {:>3}% {}/{} {}",
            cli_style::title(&bar),
            percent,
            current,
            total,
            unit
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_error_is_detectable() {
        let error: anyhow::Error = CliInterrupted.into();

        assert!(is_interrupted(&error));
    }
}
