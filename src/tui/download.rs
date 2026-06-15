use crate::tui::chrome::{create_block, draw_app_header, draw_controls_bar, draw_status_bar};
use crate::tui::layout::app_chunks;
use crate::tui::state::{AppScreen, DownloadScreen};
use crate::tui::theme::{
    tui_status_style, COLOR_ERROR, COLOR_INFO, COLOR_MUTED, COLOR_SECONDARY, COLOR_SUCCESS,
    COLOR_WARNING,
};
use msr_downloader::format;
use msr_downloader::models;
use msr_downloader::progress::{AlbumDownloadReport, DownloadProgress};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, Wrap},
};
use std::sync::{Arc, Mutex};

const DOWNLOAD_CONTROLS: &[(&str, &str)] = &[("Albums", ""), ("Help", ""), ("Quit", "")];
const DOWNLOAD_CONFIRM_CONTROLS: &[(&str, &str)] = &[("Abort", ""), ("Cancel", "")];
const DOWNLOAD_PROMPT: &str = "Tab albums | ? help | q quit | ";
const DOWNLOAD_CONFIRM_PROMPT: &str = "Abort transfer? Partial files are resumable | ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DownloadControlButton {
    Albums,
    Help,
    Quit,
    Abort,
    Cancel,
}

pub(crate) fn download_control_button_at(
    area: ratatui::layout::Rect,
    x: u16,
    y: u16,
    confirm_quit: bool,
) -> Option<DownloadControlButton> {
    if y != area.y + 1 || x < area.x + 2 {
        return None;
    }
    let prompt = if confirm_quit {
        DOWNLOAD_CONFIRM_PROMPT
    } else {
        DOWNLOAD_PROMPT
    };
    let mut cursor = area.x + 2 + prompt.len() as u16;
    for &(key, label) in download_control_items(confirm_quit) {
        let label_width = if label.trim().is_empty() {
            0
        } else {
            1 + label.trim().len() as u16
        };
        let width = 3 + key.len() as u16 + label_width;
        if x >= cursor && x < cursor.saturating_add(width) {
            return match key {
                "Albums" => Some(DownloadControlButton::Albums),
                "Help" => Some(DownloadControlButton::Help),
                "Quit" => Some(DownloadControlButton::Quit),
                "Abort" => Some(DownloadControlButton::Abort),
                "Cancel" => Some(DownloadControlButton::Cancel),
                _ => None,
            };
        }
        cursor = cursor.saturating_add(width + 1);
    }
    None
}

pub(crate) fn draw_download_screen(f: &mut ratatui::Frame, state: DownloadScreen<'_>) {
    let DownloadScreen {
        albums,
        download_queue,
        download_track_ids,
        current_album_idx,
        current,
        total,
        progress,
        reports,
        downloaded,
        done,
        confirm_quit,
    } = state;
    let chunks = app_chunks(f.area());

    let is_idle = is_transfer_idle(done, total, progress.total_songs);
    let title_color = if is_idle {
        COLOR_MUTED
    } else if done && has_done_issues(progress, reports) {
        COLOR_ERROR
    } else if done {
        COLOR_SUCCESS
    } else {
        COLOR_WARNING
    };
    let title_text = if is_idle {
        "TRANSFER IDLE / NO ACTIVE QUEUE".to_string()
    } else if done {
        if has_done_issues(progress, reports) {
            format!(
                "TRANSFER INCOMPLETE / {} OK / {} ISSUE{}",
                done_ok_count(progress, reports),
                done_issue_count(progress, reports),
                if done_issue_count(progress, reports) == 1 {
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

    let album_items: Vec<ListItem> = album_queue_rows(
        albums,
        download_queue,
        download_track_ids,
        AlbumQueueState {
            current_album_idx,
            current,
            done,
            incomplete: done && has_done_issues(progress, reports),
            is_idle,
        },
    )
    .into_iter()
    .map(|row| {
        let style = match row.state {
            AlbumQueueRowState::Incomplete => {
                Style::default().fg(COLOR_MUTED).add_modifier(Modifier::DIM)
            }
            AlbumQueueRowState::Completed => {
                Style::default().fg(COLOR_MUTED).add_modifier(Modifier::DIM)
            }
            AlbumQueueRowState::Current => Style::default()
                .fg(COLOR_SECONDARY)
                .bg(Color::Rgb(16, 20, 22))
                .add_modifier(Modifier::BOLD),
            AlbumQueueRowState::Pending => Style::default().fg(COLOR_SUCCESS),
            AlbumQueueRowState::Muted => Style::default().fg(COLOR_MUTED),
        };

        let mut spans = vec![Span::styled(format!("{} {}", row.marker, row.name), style)];
        if let Some(track_count) = row.partial_track_count {
            spans.push(Span::styled(
                format!(
                    "  {} track{}",
                    track_count,
                    if track_count == 1 { "" } else { "s" }
                ),
                Style::default().fg(COLOR_MUTED),
            ));
        }
        ListItem::new(Line::from(spans))
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
                format!("{} OK", done_ok_count(progress, reports)),
                Style::default()
                    .fg(COLOR_SUCCESS)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{} SKIPPED", done_skipped_count(progress, reports)),
                Style::default().fg(COLOR_WARNING),
            ),
            Span::raw(" / "),
            Span::styled(
                format!("{} FAILED", done_failed_count(progress, reports)),
                Style::default().fg(if done_failed_count(progress, reports) > 0 {
                    COLOR_ERROR
                } else {
                    COLOR_MUTED
                }),
            ),
        ])];

        if has_done_issues(progress, reports) {
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
            for error in done_issue_summaries(progress, reports).into_iter().take(8) {
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

    let status_text = download_status_text(
        confirm_quit,
        is_idle,
        done,
        current,
        total,
        progress,
        reports,
    );
    draw_status_bar(f, chunks[2], status_text);
    draw_controls_bar(f, chunks[3], download_controls_line(confirm_quit));
}

pub(crate) fn download_status_text(
    confirm_quit: bool,
    is_idle: bool,
    done: bool,
    current_album: usize,
    total_albums: usize,
    progress: &DownloadProgress,
    reports: &[AlbumDownloadReport],
) -> String {
    if confirm_quit {
        "ABORT CONFIRMATION: PARTIAL .part FILES WILL BE KEPT FOR RESUME".to_string()
    } else if is_idle {
        "NO ACTIVE TRANSFER".to_string()
    } else if done {
        let issue_count = done_issue_count(progress, reports);
        if issue_count > 0 {
            format!(
                "INCOMPLETE: {} OK / {} SKIPPED / {} ISSUE{}",
                done_ok_count(progress, reports),
                done_skipped_count(progress, reports),
                issue_count,
                if issue_count == 1 { "" } else { "S" }
            )
        } else {
            format!(
                "COMPLETE: {} OK / {} SKIPPED",
                done_ok_count(progress, reports),
                done_skipped_count(progress, reports)
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

fn done_ok_count(progress: &DownloadProgress, reports: &[AlbumDownloadReport]) -> usize {
    if reports.is_empty() {
        progress.ok_count()
    } else {
        reports.iter().map(AlbumDownloadReport::ok_count).sum()
    }
}

fn done_skipped_count(progress: &DownloadProgress, reports: &[AlbumDownloadReport]) -> usize {
    if reports.is_empty() {
        progress.skipped_count()
    } else {
        reports.iter().map(AlbumDownloadReport::skipped_count).sum()
    }
}

fn done_failed_count(progress: &DownloadProgress, reports: &[AlbumDownloadReport]) -> usize {
    if reports.is_empty() {
        progress.failed_count()
    } else {
        reports.iter().map(AlbumDownloadReport::failed_count).sum()
    }
}

fn done_issue_count(progress: &DownloadProgress, reports: &[AlbumDownloadReport]) -> usize {
    if reports.is_empty() {
        progress.failed_count() + progress.errors.len()
    } else {
        reports
            .iter()
            .map(|report| report.track_failure_count() + report.auxiliary_issue_count())
            .sum()
    }
}

fn has_done_issues(progress: &DownloadProgress, reports: &[AlbumDownloadReport]) -> bool {
    done_issue_count(progress, reports) > 0
}

fn done_issue_summaries(
    progress: &DownloadProgress,
    reports: &[AlbumDownloadReport],
) -> Vec<String> {
    if reports.is_empty() {
        return progress.errors.clone();
    }

    reports
        .iter()
        .flat_map(|report| report.issues.iter().map(|issue| issue.summary()))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlbumQueueRowState {
    Incomplete,
    Completed,
    Current,
    Pending,
    Muted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AlbumQueueRow<'a> {
    marker: &'static str,
    name: &'a str,
    partial_track_count: Option<usize>,
    state: AlbumQueueRowState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AlbumQueueState {
    current_album_idx: usize,
    current: usize,
    done: bool,
    incomplete: bool,
    is_idle: bool,
}

fn album_queue_rows<'a>(
    albums: &'a [models::AlbumBrief],
    download_queue: &[usize],
    download_track_ids: &[Option<Vec<String>>],
    state: AlbumQueueState,
) -> Vec<AlbumQueueRow<'a>> {
    if state.is_idle {
        return Vec::new();
    }

    let queue_pos = state.current.saturating_sub(1);
    download_queue
        .iter()
        .enumerate()
        .filter_map(|(queue_idx, album_idx)| {
            let album = albums.get(*album_idx)?;
            let is_completed = !state.incomplete && (state.done || queue_idx < queue_pos);
            let (marker, row_state) = if state.incomplete {
                ("...", AlbumQueueRowState::Incomplete)
            } else if is_completed {
                ("OK ", AlbumQueueRowState::Completed)
            } else if *album_idx == state.current_album_idx && queue_idx == queue_pos {
                ("GET", AlbumQueueRowState::Current)
            } else if queue_idx > queue_pos {
                ("...", AlbumQueueRowState::Pending)
            } else {
                ("   ", AlbumQueueRowState::Muted)
            };

            Some(AlbumQueueRow {
                marker,
                name: album.name.as_str(),
                partial_track_count: download_track_ids
                    .get(queue_idx)
                    .and_then(Option::as_ref)
                    .map(Vec::len),
                state: row_state,
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn download_controls_text(confirm_quit: bool) -> String {
    crate::tui::chrome::controls_text(download_control_items(confirm_quit))
}

fn download_controls_line(confirm_quit: bool) -> Line<'static> {
    let prompt = if confirm_quit {
        DOWNLOAD_CONFIRM_PROMPT
    } else {
        DOWNLOAD_PROMPT
    };
    let mut spans = vec![Span::raw(prompt)];
    for &(key, _) in download_control_items(confirm_quit) {
        spans.push(Span::raw("["));
        spans.push(Span::styled(key, Style::default().fg(COLOR_INFO)));
        spans.push(Span::raw("] "));
    }
    Line::from(spans)
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
    use msr_downloader::models::AlbumBrief;
    use msr_downloader::progress::SongStatus;

    fn album(name: &str) -> AlbumBrief {
        AlbumBrief {
            cid: name.to_string(),
            name: name.to_string(),
            cover_url: String::new(),
            artists: Vec::new(),
        }
    }

    #[test]
    fn download_status_stays_status_only() {
        let progress = DownloadProgress::new("", 0);

        assert_eq!(
            download_status_text(true, false, false, 1, 1, &progress, &[]),
            "ABORT CONFIRMATION: PARTIAL .part FILES WILL BE KEPT FOR RESUME"
        );
        assert_eq!(
            download_status_text(false, true, false, 1, 0, &progress, &[]),
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
            download_status_text(false, false, false, 1, 2, &progress, &[]),
            "ACTIVE: 1 TRACK / 1.0 MB/s / ETA 00:01 / 1 ALBUM LEFT / 1 TRACK LEFT"
        );
    }

    #[test]
    fn download_status_summarizes_done_transfer() {
        let mut progress = DownloadProgress::new("album", 3);
        progress.task_mut_or_insert(1, "ok", SongStatus::Done);
        progress.task_mut_or_insert(2, "skip", SongStatus::Skipped);

        assert_eq!(
            download_status_text(false, false, true, 1, 1, &progress, &[]),
            "COMPLETE: 1 OK / 1 SKIPPED"
        );

        progress.task_mut_or_insert(3, "failed", SongStatus::Failed);
        assert_eq!(
            download_status_text(false, false, true, 1, 1, &progress, &[]),
            "INCOMPLETE: 1 OK / 1 SKIPPED / 1 ISSUE"
        );
    }

    #[test]
    fn download_controls_follow_confirmation_mode() {
        assert_eq!(download_controls_text(false), "[Albums] [Help] [Quit] ");
        assert_eq!(download_controls_text(true), "[Abort] [Cancel] ");
    }

    #[test]
    fn album_queue_rows_use_download_queue_and_partial_counts() {
        let albums = vec![album("Full"), album("Partial"), album("Other")];
        let rows = album_queue_rows(
            &albums,
            &[1, 0],
            &[Some(vec!["song".to_string()]), None],
            AlbumQueueState {
                current_album_idx: 1,
                current: 1,
                done: false,
                incomplete: false,
                is_idle: false,
            },
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].marker, "GET");
        assert_eq!(rows[0].name, "Partial");
        assert_eq!(rows[0].partial_track_count, Some(1));
        assert_eq!(rows[1].marker, "...");
        assert_eq!(rows[1].name, "Full");
        assert_eq!(rows[1].partial_track_count, None);
    }

    #[test]
    fn download_control_hit_testing_uses_rendered_padding() {
        let area = ratatui::layout::Rect::new(0, 20, 100, 3);
        let albums_x = area.x + 2 + DOWNLOAD_PROMPT.len() as u16 + 1;
        let abort_x = area.x + 2 + DOWNLOAD_CONFIRM_PROMPT.len() as u16 + 1;

        assert_eq!(
            download_control_button_at(area, albums_x, area.y + 1, false),
            Some(DownloadControlButton::Albums)
        );
        assert_eq!(
            download_control_button_at(area, abort_x, area.y + 1, true),
            Some(DownloadControlButton::Abort)
        );
        assert_eq!(
            download_control_button_at(area, area.x + 1, area.y + 1, false),
            None
        );
    }
}
