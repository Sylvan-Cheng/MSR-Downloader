use crate::format;
use crate::models;
use crate::progress::DownloadProgress;
use crate::tui::chrome::{
    controls_line, create_block, draw_app_header, draw_controls_bar, draw_status_bar,
};
use crate::tui::layout::app_chunks;
use crate::tui::state::{AppScreen, DownloadScreen};
use crate::tui::theme::{
    tui_status_style, COLOR_ERROR, COLOR_INFO, COLOR_MUTED, COLOR_SECONDARY, COLOR_SUCCESS,
    COLOR_WARNING,
};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
};
use std::sync::{Arc, Mutex};

const DOWNLOAD_CONTROLS: &[(&str, &str)] =
    &[("Tab", " SWITCH  "), ("?", " HELP  "), ("Q", " QUIT")];
const DOWNLOAD_CONFIRM_CONTROLS: &[(&str, &str)] =
    &[("Y", " ABORT  "), ("N", " CANCEL  "), ("Esc", " CANCEL")];

pub(crate) fn draw_download_screen(f: &mut ratatui::Frame, state: DownloadScreen<'_>) {
    let DownloadScreen {
        albums,
        selected_albums,
        current_album_idx,
        current,
        total,
        progress,
        downloaded,
        done,
        confirm_quit,
    } = state;
    let chunks = app_chunks(f.area());

    let is_idle = is_transfer_idle(done, total, progress.total_songs);
    let title_color = if is_idle {
        COLOR_MUTED
    } else if done && (progress.failed_count() > 0 || !progress.errors.is_empty()) {
        COLOR_ERROR
    } else if done {
        COLOR_SUCCESS
    } else {
        COLOR_WARNING
    };
    let title_text = if is_idle {
        "TRANSFER IDLE / NO ACTIVE QUEUE".to_string()
    } else if done {
        if progress.failed_count() > 0 || !progress.errors.is_empty() {
            format!(
                "TRANSFER INCOMPLETE / {} OK / {} ISSUE{}",
                progress.ok_count(),
                progress.failed_count() + progress.errors.len(),
                if progress.failed_count() + progress.errors.len() == 1 {
                    ""
                } else {
                    "S"
                }
            )
        } else {
            format!(
                "TRANSFER COMPLETE / {} ALBUM{} ARCHIVED",
                downloaded.len(),
                if downloaded.len() == 1 { "" } else { "S" }
            )
        }
    } else {
        format!(
            "ALBUM {}/{} / TRACKS {}/{} / {}",
            current, total, progress.completed_songs, progress.total_songs, progress.album_name
        )
    };
    draw_app_header(
        f,
        chunks[0],
        AppScreen::Downloading,
        title_text,
        title_color,
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(chunks[1]);

    let album_items: Vec<ListItem> = albums
        .iter()
        .enumerate()
        .filter(|(idx, _)| !is_idle && (selected_albums[*idx] || *idx == current_album_idx))
        .map(|(i, a)| {
            let incomplete = done && (progress.failed_count() > 0 || !progress.errors.is_empty());
            let queue_pos = current.saturating_sub(1);
            let album_pos = selected_albums[..i]
                .iter()
                .filter(|&&selected| selected)
                .count();
            let is_completed = !incomplete && (done || album_pos < queue_pos);
            let (marker, style) = if incomplete {
                (
                    "...",
                    Style::default().fg(COLOR_MUTED).add_modifier(Modifier::DIM),
                )
            } else if is_completed {
                (
                    "OK ",
                    Style::default().fg(COLOR_MUTED).add_modifier(Modifier::DIM),
                )
            } else if i == current_album_idx {
                (
                    "GET",
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .bg(Color::Rgb(16, 20, 22))
                        .add_modifier(Modifier::BOLD),
                )
            } else if selected_albums[i] {
                ("...", Style::default().fg(COLOR_SUCCESS))
            } else {
                ("   ", Style::default().fg(COLOR_MUTED))
            };

            ListItem::new(Line::from(vec![Span::styled(
                format!("{} {}", marker, a.name),
                style,
            )]))
        })
        .collect();

    let album_list = List::new(album_items).block(create_block("ALBUM QUEUE", COLOR_MUTED));
    f.render_widget(album_list, body[0]);

    if is_idle {
        let idle = Paragraph::new(Line::from(vec![
            Span::styled("NO TRANSFER QUEUE", Style::default().fg(COLOR_MUTED)),
            Span::raw("  /  "),
            Span::styled("TAB", Style::default().fg(COLOR_INFO)),
            Span::raw(" OR "),
            Span::styled("1", Style::default().fg(COLOR_INFO)),
            Span::raw(" BACK TO ALBUMS"),
        ]))
        .block(create_block("TRANSFER SUMMARY", COLOR_MUTED));
        f.render_widget(idle, body[1]);
    } else if done {
        let mut lines = vec![Line::from(vec![
            Span::styled("TRACKS  ", Style::default().fg(COLOR_MUTED)),
            Span::styled(
                format!("{} OK", progress.ok_count()),
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{} SKIPPED", progress.skipped_count()),
                Style::default().fg(COLOR_WARNING),
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{} FAILED", progress.failed_count()),
                Style::default().fg(if progress.failed_count() > 0 {
                    COLOR_ERROR
                } else {
                    COLOR_MUTED
                }),
            ),
        ])];

        if progress.failed_count() > 0 || !progress.errors.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "ISSUES",
                Style::default()
                    .fg(COLOR_ERROR)
                    .add_modifier(Modifier::BOLD),
            )));
            for task in progress
                .tasks
                .iter()
                .filter(|task| task.is_failed())
                .take(8)
            {
                lines.push(Line::from(vec![
                    Span::styled("ERR ", Style::default().fg(COLOR_ERROR)),
                    Span::raw(task.name.clone()),
                ]));
            }
            for error in progress.errors.iter().take(8) {
                lines.push(Line::from(vec![
                    Span::styled("ERR ", Style::default().fg(COLOR_ERROR)),
                    Span::raw(error.clone()),
                ]));
            }
        } else {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "ARCHIVE COMPLETE",
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let summary = Paragraph::new(lines)
            .block(create_block("TRANSFER SUMMARY", COLOR_MUTED))
            .wrap(Wrap { trim: true });
        f.render_widget(summary, body[1]);
    } else {
        let mut tasks = progress.tasks.clone();
        tasks.sort_by_key(|task| task.index);
        let task_items: Vec<ListItem> = tasks
            .iter()
            .map(|task| {
                let ratio = format::progress_ratio(task.bytes_downloaded, task.total_bytes);
                let bar = format::progress_bar(ratio, 24);
                let status = Span::styled(task.status.code(), tui_status_style(task.status));

                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(format!(" {:>2}/{:<2} ", task.index, progress.total_songs)),
                    Span::styled(bar, Style::default().fg(COLOR_INFO)),
                    Span::raw(format!(" {:>3}% ", (ratio * 100.0).round() as u64)),
                    Span::styled(
                        format!(
                            "{}/{} ",
                            format::format_bytes(task.bytes_downloaded),
                            format::format_bytes(task.total_bytes)
                        ),
                        Style::default().fg(COLOR_SECONDARY),
                    ),
                    Span::styled(
                        format!("{}/s ", format::format_bytes(task.speed_bps as u64)),
                        Style::default().fg(COLOR_MUTED),
                    ),
                    Span::raw(task.name.clone()),
                ]))
            })
            .collect();

        let task_title = format!(
            "TRACKS {}/{}",
            progress.completed_songs, progress.total_songs
        );
        if task_items.is_empty() {
            let placeholder = Paragraph::new("PREPARING TRANSFERS...")
                .block(create_block(&task_title, COLOR_MUTED));
            f.render_widget(placeholder, body[1]);
        } else {
            let task_list = List::new(task_items).block(create_block(&task_title, COLOR_MUTED));
            f.render_widget(task_list, body[1]);
        }
    }

    let status_text = download_status_text(confirm_quit, is_idle, done, current, total, progress);
    draw_status_bar(f, chunks[2], status_text);
    draw_controls_bar(
        f,
        chunks[3],
        controls_line(download_control_items(confirm_quit)),
    );
}

pub(crate) fn download_status_text(
    confirm_quit: bool,
    is_idle: bool,
    done: bool,
    current_album: usize,
    total_albums: usize,
    progress: &DownloadProgress,
) -> String {
    if confirm_quit {
        "ABORT CONFIRMATION: PARTIAL .part FILES WILL BE KEPT FOR RESUME".to_string()
    } else if is_idle {
        "NO ACTIVE TRANSFER".to_string()
    } else if done {
        let issue_count = progress.failed_count() + progress.errors.len();
        if issue_count > 0 {
            format!(
                "INCOMPLETE: {} OK / {} SKIPPED / {} ISSUE{}",
                progress.ok_count(),
                progress.skipped_count(),
                issue_count,
                if issue_count == 1 { "" } else { "S" }
            )
        } else {
            format!(
                "COMPLETE: {} OK / {} SKIPPED",
                progress.ok_count(),
                progress.skipped_count()
            )
        }
    } else if let Some(error) = progress.errors.last() {
        format!("LAST ERROR: {}", error)
    } else {
        let albums_left = total_albums.saturating_sub(current_album);
        let tracks_left = progress
            .total_songs
            .saturating_sub(progress.completed_songs);
        format!(
            "ACTIVE: {} TRACK{} / {}/s / ETA {} / {} ALBUM{} LEFT / {} TRACK{} LEFT",
            progress.active_count(),
            if progress.active_count() == 1 {
                ""
            } else {
                "S"
            },
            format::format_bytes(progress.total_speed_bps() as u64),
            progress
                .eta_seconds()
                .map(format::format_duration)
                .unwrap_or_else(|| "--:--".to_string()),
            albums_left,
            if albums_left == 1 { "" } else { "S" },
            tracks_left,
            if tracks_left == 1 { "" } else { "S" }
        )
    }
}

#[cfg(test)]
pub(crate) fn download_controls_text(confirm_quit: bool) -> String {
    crate::tui::chrome::controls_text(download_control_items(confirm_quit))
}

fn download_control_items(confirm_quit: bool) -> &'static [(&'static str, &'static str)] {
    if confirm_quit {
        DOWNLOAD_CONFIRM_CONTROLS
    } else {
        DOWNLOAD_CONTROLS
    }
}

pub(crate) fn is_transfer_idle(done: bool, total_albums: usize, total_songs: usize) -> bool {
    !done && total_albums == 0 && total_songs == 0
}

pub(crate) fn current_transfer_index(
    download_queue: &[usize],
    albums: &[models::AlbumBrief],
    progress: &Arc<Mutex<DownloadProgress>>,
) -> Option<(usize, usize)> {
    let album_name = progress.lock().ok()?.album_name.clone();
    download_queue
        .iter()
        .enumerate()
        .find(|(_, album_idx)| albums[**album_idx].name == album_name)
        .map(|(queue_idx, album_idx)| (queue_idx, *album_idx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::SongStatus;

    #[test]
    fn download_status_stays_status_only() {
        let progress = DownloadProgress::new("", 0);

        assert_eq!(
            download_status_text(true, false, false, 1, 1, &progress),
            "ABORT CONFIRMATION: PARTIAL .part FILES WILL BE KEPT FOR RESUME"
        );
        assert_eq!(
            download_status_text(false, true, false, 1, 0, &progress),
            "NO ACTIVE TRANSFER"
        );
    }

    #[test]
    fn download_status_summarizes_active_transfer() {
        let mut progress = DownloadProgress::new("album", 2);
        progress.completed_songs = 1;
        let task = progress.task_mut_or_insert(1, "song", SongStatus::Getting);
        task.bytes_downloaded = 1024 * 1024;
        task.total_bytes = 2 * 1024 * 1024;
        task.speed_bps = 1024.0 * 1024.0;

        assert_eq!(
            download_status_text(false, false, false, 1, 2, &progress),
            "ACTIVE: 1 TRACK / 1.0 MB/s / ETA 00:01 / 1 ALBUM LEFT / 1 TRACK LEFT"
        );
    }

    #[test]
    fn download_status_summarizes_done_transfer() {
        let mut progress = DownloadProgress::new("album", 3);
        progress.task_mut_or_insert(1, "ok", SongStatus::Done);
        progress.task_mut_or_insert(2, "skip", SongStatus::Skipped);

        assert_eq!(
            download_status_text(false, false, true, 1, 1, &progress),
            "COMPLETE: 1 OK / 1 SKIPPED"
        );

        progress.task_mut_or_insert(3, "failed", SongStatus::Failed);
        assert_eq!(
            download_status_text(false, false, true, 1, 1, &progress),
            "INCOMPLETE: 1 OK / 1 SKIPPED / 1 ISSUE"
        );
    }

    #[test]
    fn download_controls_follow_confirmation_mode() {
        assert_eq!(download_controls_text(false), "Tab SWITCH  ? HELP  Q QUIT");
        assert_eq!(
            download_controls_text(true),
            "Y ABORT  N CANCEL  Esc CANCEL"
        );
    }
}
