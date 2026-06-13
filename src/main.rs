mod api;
mod config;
mod downloader;
mod metadata;
mod models;

use api::ApiClient;
use clap::Parser;
use config::Config;
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, size as terminal_size, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use downloader::{DownloadProgress, SongStatus};
use owo_colors::OwoColorize;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            cursor::Show
        );
    }
}

fn is_key(code: KeyCode, expected: char) -> bool {
    matches!(code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&expected))
}

const COLOR_PRIMARY: Color = Color::Rgb(0, 216, 198);
const COLOR_SECONDARY: Color = Color::Rgb(214, 218, 216);
const COLOR_SUCCESS: Color = Color::Rgb(0, 216, 198);
const COLOR_WARNING: Color = Color::Rgb(214, 218, 216);
const COLOR_ERROR: Color = Color::Rgb(238, 89, 82);
const COLOR_INFO: Color = Color::Rgb(0, 216, 198);
const COLOR_MUTED: Color = Color::Rgb(92, 98, 100);

#[derive(Parser)]
#[command(name = "msr-downloader", about = "Monster Siren Music Downloader")]
struct Cli {
    #[arg(short, long)]
    config: Option<PathBuf>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, num_args = 1..)]
    album: Option<Vec<String>>,
    #[arg(short, long)]
    list: bool,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    cli: bool,
    #[arg(long)]
    plain: bool,
    #[arg(long)]
    no_progress: bool,
    #[arg(long)]
    concurrency: Option<usize>,
    #[arg(long)]
    print_config: bool,
    #[arg(long)]
    clean_parts: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    yes: bool,
}

fn print_config_summary(config: &Config) {
    println!("MSR// CONFIG");
    println!("  api.base_url = {}", config.api.base_url);
    println!("  api.timeout = {}", config.api.timeout);
    println!(
        "  download.output_dir = {}",
        config.download.output_dir.display()
    );
    println!("  download.concurrency = {}", config.download.concurrency);
    println!("  include.lyrics = {}", config.download.include.lyrics);
    println!("  include.covers = {}", config.download.include.covers);
    println!(
        "  include.album_info = {}",
        config.download.include.album_info
    );
    println!("  include.metadata = {}", config.download.include.metadata);
    println!("  convert.enabled = {}", config.download.convert.enabled);
    println!(
        "  convert.wav_to_flac = {}",
        config.download.convert.wav_to_flac
    );
}

fn collect_partial_files(dir: &std::path::Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.try_exists()? {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            collect_partial_files(&path, files)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".part"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn clean_partial_files(dir: &std::path::Path, dry_run: bool, yes: bool) -> anyhow::Result<usize> {
    let mut partial_files = Vec::new();
    collect_partial_files(dir, &mut partial_files)?;

    println!(
        "{} SCANNED {} / {} PARTIAL FILE{} FOUND",
        "MSR//".cyan().bold(),
        dir.display(),
        partial_files.len(),
        if partial_files.len() == 1 { "" } else { "S" }
    );

    if dry_run {
        for file in &partial_files {
            println!("  {}", file.display());
        }
        return Ok(0);
    }

    if !partial_files.is_empty() && !yes {
        anyhow::bail!("refusing to delete partial files without --yes; use --dry-run to preview");
    }

    for file in &partial_files {
        std::fs::remove_file(file)?;
    }
    Ok(partial_files.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppScreen {
    Select,
    Downloading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AlbumMouseAction {
    Focus(usize),
    Toggle(usize),
}

struct DownloadScreen<'a> {
    albums: &'a [models::AlbumBrief],
    selected_albums: &'a [bool],
    current_album_idx: usize,
    current: usize,
    total: usize,
    progress: &'a DownloadProgress,
    downloaded: &'a [String],
    done: bool,
    confirm_quit: bool,
}

fn filtered_album_indices(albums: &[models::AlbumBrief], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    albums
        .iter()
        .enumerate()
        .filter(|(_, album)| query.is_empty() || album.name.to_lowercase().contains(&query))
        .map(|(idx, _)| idx)
        .collect()
}

fn selected_visible_position(visible_indices: &[usize], selected: usize) -> usize {
    visible_indices
        .iter()
        .position(|&idx| idx == selected)
        .unwrap_or(0)
}

fn ensure_visible_selection(selected: &mut usize, visible_indices: &[usize]) {
    if visible_indices.is_empty() {
        return;
    }
    if !visible_indices.contains(selected) {
        *selected = visible_indices[0];
    }
}

fn move_selection(selected: &mut usize, visible_indices: &[usize], delta: isize) {
    if visible_indices.is_empty() {
        return;
    }
    let current = selected_visible_position(visible_indices, *selected);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        (current + delta as usize).min(visible_indices.len() - 1)
    };
    *selected = visible_indices[next];
}

fn create_block(title: &str, border_color: Color) -> Block<'_> {
    Block::default()
        .title(format!(" {} ", title.to_ascii_uppercase()))
        .title_style(
            Style::default()
                .fg(COLOR_SECONDARY)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1))
}

fn progress_bar(ratio: f64, width: usize) -> String {
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
    format!("{}{}{}", "─".repeat(filled), head, "·".repeat(empty))
}

fn tui_status_style(status: SongStatus) -> Style {
    match status {
        SongStatus::Failed => Style::default()
            .fg(COLOR_ERROR)
            .add_modifier(Modifier::BOLD),
        SongStatus::Skipped => Style::default()
            .fg(COLOR_WARNING)
            .add_modifier(Modifier::BOLD),
        SongStatus::Done | SongStatus::Resuming => Style::default()
            .fg(COLOR_SUCCESS)
            .add_modifier(Modifier::BOLD),
        SongStatus::Checking | SongStatus::Tagging => Style::default()
            .fg(COLOR_SECONDARY)
            .add_modifier(Modifier::BOLD),
        SongStatus::Queued => Style::default().fg(COLOR_MUTED),
        SongStatus::Getting => Style::default().fg(COLOR_INFO).add_modifier(Modifier::BOLD),
    }
}

fn tab_span(label: &'static str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            label,
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label, Style::default().fg(COLOR_MUTED))
    }
}

fn draw_app_header(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    screen: AppScreen,
    title_text: String,
    title_color: Color,
) {
    let title = Paragraph::new(Line::from(vec![
        tab_span("ALBUMS [1]", screen == AppScreen::Select),
        Span::raw("    "),
        tab_span("TRANSFER [2]", screen == AppScreen::Downloading),
        Span::raw("    "),
        Span::styled(
            title_text,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(create_block("MONSTER SIREN RECORDS", title_color));
    f.render_widget(title, area);
}

fn draw_status_bar(f: &mut ratatui::Frame, area: ratatui::layout::Rect, text: String) {
    let status = Paragraph::new(text)
        .style(Style::default().fg(COLOR_MUTED))
        .block(create_block("STATUS", COLOR_MUTED));
    f.render_widget(status, area);
}

fn draw_controls_bar(f: &mut ratatui::Frame, area: ratatui::layout::Rect, line: Line<'static>) {
    let controls = Paragraph::new(line).block(create_block("CONTROLS", COLOR_MUTED));
    f.render_widget(controls, area);
}

fn draw_select_screen(
    f: &mut ratatui::Frame,
    albums: &[models::AlbumBrief],
    selected: usize,
    selected_albums: &[bool],
    is_downloading: bool,
    search_query: &str,
    search_active: bool,
) {
    let chunks = app_chunks(f.area());

    draw_app_header(
        f,
        chunks[0],
        AppScreen::Select,
        "WELCOME TO A WORLD FAMILIARLY UNKNOWN".to_string(),
        COLOR_PRIMARY,
    );

    let body = select_body_chunks(chunks[1]);

    let visible_indices = filtered_album_indices(albums, search_query);
    let visible_position = selected_visible_position(&visible_indices, selected);

    let items: Vec<ListItem> = visible_indices
        .iter()
        .map(|&i| {
            let a = &albums[i];
            let (checkbox, style) = if i == selected {
                (
                    ">",
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .bg(Color::Rgb(16, 20, 22))
                        .add_modifier(Modifier::BOLD),
                )
            } else if selected_albums[i] {
                ("+", Style::default().fg(COLOR_SUCCESS))
            } else {
                ("-", Style::default().fg(COLOR_MUTED))
            };

            ListItem::new(Line::from(vec![Span::styled(
                format!("{} {}", checkbox, a.name),
                style,
            )]))
        })
        .collect();

    let list_title = if search_query.is_empty() {
        "BROWSE ALBUMS".to_string()
    } else {
        format!("BROWSE ALBUMS / FILTER {} MATCHES", visible_indices.len())
    };
    let list_block = create_block(&list_title, COLOR_MUTED);
    let list = List::new(items)
        .block(list_block)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let mut list_state = ListState::default();
    if !visible_indices.is_empty() {
        list_state.select(Some(visible_position));
    }
    f.render_stateful_widget(list, body[0], &mut list_state);

    let queue_items: Vec<ListItem> = albums
        .iter()
        .enumerate()
        .filter(|(idx, _)| selected_albums[*idx])
        .map(|(_, album)| {
            ListItem::new(Line::from(vec![
                Span::styled("+ ", Style::default().fg(COLOR_PRIMARY)),
                Span::styled(album.name.clone(), Style::default().fg(COLOR_SECONDARY)),
            ]))
        })
        .collect();

    if queue_items.is_empty() {
        let empty_queue = Paragraph::new(Line::from(vec![
            Span::styled("SPACE", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(" TO ADD AN ALBUM\n"),
            Span::styled("ENTER", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(" TO START WHEN READY"),
        ]))
        .style(Style::default().fg(COLOR_MUTED))
        .block(create_block("DOWNLOAD QUEUE", COLOR_MUTED));
        f.render_widget(empty_queue, body[1]);
    } else {
        let queue = List::new(queue_items).block(create_block("DOWNLOAD QUEUE", COLOR_PRIMARY));
        f.render_widget(queue, body[1]);
    }

    // Status bar
    let count = selected_albums.iter().filter(|&&s| s).count();
    let status_text = if search_active {
        format!("FILTER: {}", search_query)
    } else if is_downloading {
        format!("TRANSFER ACTIVE / {} IN QUEUE / TAB TO PROGRESS", count)
    } else if count == 0 {
        "NO ALBUM SELECTED".to_string()
    } else {
        format!("{} ALBUM{} READY", count, if count > 1 { "S" } else { "" })
    };
    draw_status_bar(f, chunks[2], status_text);

    draw_controls_bar(
        f,
        chunks[3],
        Line::from(vec![
            Span::styled("↑↓", Style::default().fg(COLOR_INFO)),
            Span::raw(" MOVE  "),
            Span::styled("Space", Style::default().fg(COLOR_INFO)),
            Span::raw(" SELECT  "),
            Span::styled("A", Style::default().fg(COLOR_INFO)),
            Span::raw(" ALL  "),
            Span::styled("C", Style::default().fg(COLOR_INFO)),
            Span::raw(" CLEAR  "),
            Span::styled("/", Style::default().fg(COLOR_INFO)),
            Span::raw(" SEARCH  "),
            Span::styled("Esc", Style::default().fg(COLOR_INFO)),
            Span::raw(" CLEAR FILTER  "),
            Span::styled("Enter", Style::default().fg(COLOR_INFO)),
            Span::raw(" DOWNLOAD  "),
            Span::styled("Tab", Style::default().fg(COLOR_INFO)),
            Span::raw(" PROGRESS  "),
            Span::styled("Q", Style::default().fg(COLOR_INFO)),
            Span::raw(" QUIT"),
        ]),
    );
}

fn draw_download_screen(f: &mut ratatui::Frame, state: DownloadScreen<'_>) {
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
            let queue_pos = current.saturating_sub(1);
            let album_pos = selected_albums[..i]
                .iter()
                .filter(|&&selected| selected)
                .count();
            let is_completed = done || album_pos < queue_pos;
            let (marker, style) = if is_completed {
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
            for task in progress.tasks.iter().filter(|task| task.failed).take(8) {
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

        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("TAB", Style::default().fg(COLOR_INFO)),
            Span::raw(" BACK TO ALBUMS  "),
            Span::styled("Q", Style::default().fg(COLOR_INFO)),
            Span::raw(" QUIT"),
        ]));

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
                let ratio = downloader::progress_ratio(task.bytes_downloaded, task.total_bytes);
                let bar = progress_bar(ratio, 24);
                let status = Span::styled(task.status.code(), tui_status_style(task.status));

                ListItem::new(Line::from(vec![
                    status,
                    Span::raw(format!(" {:>2}/{:<2} ", task.index, progress.total_songs)),
                    Span::styled(bar, Style::default().fg(COLOR_INFO)),
                    Span::raw(format!(" {:>3}% ", (ratio * 100.0).round() as u64)),
                    Span::styled(
                        format!(
                            "{}/{} ",
                            downloader::format_bytes(task.bytes_downloaded),
                            downloader::format_bytes(task.total_bytes)
                        ),
                        Style::default().fg(COLOR_SECONDARY),
                    ),
                    Span::styled(
                        format!("{}/s ", downloader::format_bytes(task.speed_bps as u64)),
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

    let status_text = if confirm_quit {
        "ABORT ACTIVE DOWNLOAD?  Y CONFIRM  N/ESC CANCEL  /  PARTIAL .part FILES ARE KEPT FOR RESUME".to_string()
    } else if is_idle {
        "ALBUMS [1]  TRANSFER [2]  TAB SWITCH  Q QUIT  /  NO ACTIVE TRANSFER".to_string()
    } else if done {
        if progress.failed_count() > 0 || !progress.errors.is_empty() {
            format!(
                "TAB ALBUMS  Q QUIT  /  TRANSFER INCOMPLETE  /  {} OK  {} SKIPPED  {} ISSUE{}",
                progress.ok_count(),
                progress.skipped_count(),
                progress.failed_count() + progress.errors.len(),
                if progress.failed_count() + progress.errors.len() == 1 {
                    ""
                } else {
                    "S"
                }
            )
        } else {
            format!(
                "TAB ALBUMS  Q QUIT  /  TRANSFER COMPLETE  /  {} OK  {} SKIPPED",
                progress.ok_count(),
                progress.skipped_count()
            )
        }
    } else if let Some(error) = progress.errors.last() {
        format!("TAB ALBUMS  Q QUIT  /  LAST ERROR: {}", error)
    } else {
        format!(
            "ALBUMS [1]  TRANSFER [2]  TAB SWITCH  Q QUIT  /  {} ACTIVE  {}/s  ETA {}  /  {} ALBUM{} LEFT  /  {} TRACK{} LEFT",
            progress.active_count(),
            downloader::format_bytes(progress.total_speed_bps() as u64),
            progress
                .eta_seconds()
                .map(downloader::format_duration)
                .unwrap_or_else(|| "--:--".to_string()),
            total.saturating_sub(current),
            if total.saturating_sub(current) == 1 {
                ""
            } else {
                "S"
            },
            progress
                .total_songs
                .saturating_sub(progress.completed_songs),
            if progress
                .total_songs
                .saturating_sub(progress.completed_songs)
                == 1
            {
                ""
            } else {
                "S"
            }
        )
    };
    draw_status_bar(f, chunks[2], status_text);
    draw_controls_bar(
        f,
        chunks[3],
        Line::from(vec![
            Span::styled("1", Style::default().fg(COLOR_INFO)),
            Span::raw(" ALBUMS  "),
            Span::styled("2", Style::default().fg(COLOR_INFO)),
            Span::raw(" TRANSFER  "),
            Span::styled("Tab", Style::default().fg(COLOR_INFO)),
            Span::raw(" SWITCH  "),
            Span::styled("Q", Style::default().fg(COLOR_INFO)),
            Span::raw(if confirm_quit { " ABORT? Y/N" } else { " QUIT" }),
        ]),
    );
}

fn is_transfer_idle(done: bool, total_albums: usize, total_songs: usize) -> bool {
    !done && total_albums == 0 && total_songs == 0
}

fn current_transfer_index(
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

fn app_chunks(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(area)
}

fn select_body_chunks(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
        .split(area)
}

fn contains_point(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x
        && x < rect.x.saturating_add(rect.width)
        && y >= rect.y
        && y < rect.y.saturating_add(rect.height)
}

fn screen_from_header_click(header: Rect, x: u16, y: u16) -> Option<AppScreen> {
    if !contains_point(header, x, y) || y != header.y + 1 {
        return None;
    }

    let content_x = header.x + 1;
    let albums_start = content_x;
    let albums_end = albums_start + "ALBUMS [1]".len() as u16;
    let transfer_start = albums_end + 4;
    let transfer_end = transfer_start + "TRANSFER [2]".len() as u16;

    if x >= albums_start && x < albums_end {
        Some(AppScreen::Select)
    } else if x >= transfer_start && x < transfer_end {
        Some(AppScreen::Downloading)
    } else {
        None
    }
}

fn album_mouse_action(
    selected_visible: usize,
    album_count: usize,
    list_area: Rect,
    mouse_x: u16,
    mouse_y: u16,
) -> Option<AlbumMouseAction> {
    if album_count == 0 || !contains_point(list_area, mouse_x, mouse_y) {
        return None;
    }

    let content_left = list_area.x.saturating_add(2);
    let content_top = list_area.y.saturating_add(1);
    let content_bottom = list_area
        .y
        .saturating_add(list_area.height)
        .saturating_sub(1);
    if mouse_y < content_top || mouse_y >= content_bottom || mouse_x < content_left {
        return None;
    }

    let visible_rows = content_bottom.saturating_sub(content_top) as usize;
    if visible_rows == 0 {
        return None;
    }

    let start = selected_visible
        .saturating_add(1)
        .saturating_sub(visible_rows);
    let row = mouse_y.saturating_sub(content_top) as usize;
    let index = start + row;
    if index >= album_count {
        return None;
    }

    if mouse_x <= content_left + 1 {
        Some(AlbumMouseAction::Toggle(index))
    } else {
        Some(AlbumMouseAction::Focus(index))
    }
}

async fn run_tui(api: &ApiClient, config: &Config) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let _terminal_guard = TerminalGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut screen = AppScreen::Select;
    let mut selected = 0usize;
    let mut selected_albums: Vec<bool> = vec![false; albums.len()];
    let mut search_query = String::new();
    let mut search_active = false;
    let mut downloaded_names: Vec<String> = Vec::new();

    let mut download_queue: Vec<usize> = Vec::new();
    let mut download_current = 0usize;
    let mut transfer_done = false;
    let mut active_album_idx = 0usize;
    let mut confirm_quit = false;
    let mut download_handle: Option<JoinHandle<anyhow::Result<Vec<String>>>> = None;

    let progress = Arc::new(Mutex::new(DownloadProgress::new("", 0)));

    loop {
        if let Some(handle) = download_handle.as_ref() {
            if handle.is_finished() {
                if let Some(handle) = download_handle.take() {
                    match handle.await {
                        Ok(Ok(names)) => {
                            downloaded_names = names;
                        }
                        Ok(Err(e)) => {
                            if let Ok(mut prog) = progress.lock() {
                                prog.errors.push(format!("Download error: {}", e));
                            }
                        }
                        Err(e) => {
                            downloaded_names.clear();
                            if let Ok(mut prog) = progress.lock() {
                                prog.errors.push(format!("Task error: {}", e));
                            }
                        }
                    }
                    transfer_done = true;
                }
            }
        }

        match screen {
            AppScreen::Select => {
                let visible_indices = filtered_album_indices(&albums, &search_query);
                ensure_visible_selection(&mut selected, &visible_indices);

                terminal.draw(|f| {
                    draw_select_screen(
                        f,
                        &albums,
                        selected,
                        &selected_albums,
                        download_handle.is_some() || transfer_done,
                        &search_query,
                        search_active,
                    );
                })?;

                if event::poll(std::time::Duration::from_millis(50))? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                            KeyCode::Esc => {
                                search_active = false;
                                search_query.clear();
                            }
                            KeyCode::Backspace if search_active => {
                                search_query.pop();
                            }
                            KeyCode::Char('/') if !search_active => {
                                search_active = true;
                            }
                            KeyCode::Char(ch) if search_active => {
                                search_query.push(ch);
                            }
                            code if is_key(code, 'q') => {
                                if download_handle.is_some() && !transfer_done {
                                    confirm_quit = true;
                                    screen = AppScreen::Downloading;
                                } else {
                                    break;
                                }
                            }
                            KeyCode::Char('1') => screen = AppScreen::Select,
                            KeyCode::Char('2') => screen = AppScreen::Downloading,
                            KeyCode::Tab => screen = AppScreen::Downloading,
                            KeyCode::Up => {
                                move_selection(&mut selected, &visible_indices, -1);
                            }
                            KeyCode::Down => {
                                move_selection(&mut selected, &visible_indices, 1);
                            }
                            KeyCode::Char(' ') => {
                                if !visible_indices.is_empty() && download_handle.is_none() {
                                    selected_albums[selected] = !selected_albums[selected];
                                }
                            }
                            code if is_key(code, 'a') && download_handle.is_none() => {
                                let all_selected =
                                    visible_indices.iter().all(|&idx| selected_albums[idx]);
                                for &idx in &visible_indices {
                                    selected_albums[idx] = !all_selected;
                                }
                            }
                            code if is_key(code, 'c') && download_handle.is_none() => {
                                selected_albums.fill(false);
                            }
                            KeyCode::Enter if download_handle.is_none() => {
                                download_queue = selected_albums
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, &s)| s)
                                    .map(|(i, _)| i)
                                    .collect();

                                if !download_queue.is_empty() {
                                    download_current = 0;
                                    downloaded_names.clear();
                                    transfer_done = false;
                                    confirm_quit = false;
                                    active_album_idx = download_queue[0];
                                    *progress.lock().expect("progress lock poisoned") =
                                        DownloadProgress::new("Preparing...", 0);

                                    let api_clone = api.clone();
                                    let config_clone = config.clone();
                                    let progress_clone = progress.clone();
                                    let queued_albums: Vec<_> = download_queue
                                        .iter()
                                        .map(|&idx| (idx, albums[idx].clone()))
                                        .collect();
                                    download_handle = Some(tokio::spawn(async move {
                                        let mut downloaded = Vec::new();
                                        let mut failures = Vec::new();
                                        for (_, album) in queued_albums {
                                            let album_detail = match api_clone
                                                .get_album_detail(&album.cid)
                                                .await
                                            {
                                                Ok(album_detail) => album_detail,
                                                Err(e) => {
                                                    let message = format!(
                                                        "Album {} detail error: {}",
                                                        album.name, e
                                                    );
                                                    if let Ok(mut prog) = progress_clone.lock() {
                                                        prog.errors.push(message.clone());
                                                    }
                                                    failures.push(message);
                                                    continue;
                                                }
                                            };
                                            if let Ok(mut prog) = progress_clone.lock() {
                                                *prog = DownloadProgress::new(
                                                    &album_detail.name,
                                                    album_detail.songs.len(),
                                                );
                                            }
                                            downloader::download_album_with_progress(
                                                &api_clone,
                                                &album_detail,
                                                &config_clone,
                                                Some(progress_clone.clone()),
                                            )
                                            .await
                                            .map(|_| downloaded.push(album.name.clone()))
                                            .unwrap_or_else(|e| {
                                                let message = format!(
                                                    "Album {} download error: {}",
                                                    album.name, e
                                                );
                                                if let Ok(mut prog) = progress_clone.lock() {
                                                    prog.errors.push(message.clone());
                                                }
                                                failures.push(message);
                                            });
                                        }
                                        if !failures.is_empty() {
                                            if let Ok(mut prog) = progress_clone.lock() {
                                                for failure in &failures {
                                                    if !prog.errors.contains(failure) {
                                                        prog.errors.push(failure.clone());
                                                    }
                                                }
                                            }
                                            anyhow::bail!(
                                                "{} album(s) failed: {}",
                                                failures.len(),
                                                failures.join("; ")
                                            );
                                        }
                                        Ok(downloaded)
                                    }));
                                    screen = AppScreen::Downloading;
                                }
                            }
                            KeyCode::Enter => {}
                            _ => {}
                        },
                        Event::Mouse(mouse) => match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    if contains_point(body[0], mouse.column, mouse.row) {
                                        move_selection(&mut selected, &visible_indices, -1);
                                    }
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    if contains_point(body[0], mouse.column, mouse.row) {
                                        move_selection(&mut selected, &visible_indices, 1);
                                    }
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    if let Some(next_screen) =
                                        screen_from_header_click(chunks[0], mouse.column, mouse.row)
                                    {
                                        screen = next_screen;
                                    } else {
                                        let body = select_body_chunks(chunks[1]);
                                        if let Some(action) = album_mouse_action(
                                            selected_visible_position(&visible_indices, selected),
                                            visible_indices.len(),
                                            body[0],
                                            mouse.column,
                                            mouse.row,
                                        ) {
                                            let index = match action {
                                                AlbumMouseAction::Focus(index)
                                                | AlbumMouseAction::Toggle(index) => index,
                                            };
                                            if let Some(&album_index) = visible_indices.get(index) {
                                                selected = album_index;
                                                if matches!(action, AlbumMouseAction::Toggle(_))
                                                    && download_handle.is_none()
                                                {
                                                    selected_albums[selected] =
                                                        !selected_albums[selected];
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            AppScreen::Downloading => {
                if let Some((queue_idx, album_idx)) =
                    current_transfer_index(&download_queue, &albums, &progress)
                {
                    download_current = queue_idx;
                    active_album_idx = album_idx;
                }

                if let Ok(prog) = progress.lock() {
                    terminal.draw(|f| {
                        draw_download_screen(
                            f,
                            DownloadScreen {
                                albums: &albums,
                                selected_albums: &selected_albums,
                                current_album_idx: active_album_idx,
                                current: download_current + 1,
                                total: download_queue.len(),
                                progress: &prog,
                                downloaded: &downloaded_names,
                                done: transfer_done,
                                confirm_quit,
                            },
                        );
                    })?;
                }

                if event::poll(std::time::Duration::from_millis(80))? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                            code if is_key(code, 'q') => {
                                if download_handle.is_some() && !transfer_done {
                                    confirm_quit = true;
                                } else {
                                    break;
                                }
                            }
                            code if confirm_quit && is_key(code, 'y') => break,
                            code if confirm_quit && (is_key(code, 'n') || code == KeyCode::Esc) => {
                                confirm_quit = false;
                            }
                            KeyCode::Char('1') => {
                                if transfer_done {
                                    selected_albums.fill(false);
                                }
                                screen = AppScreen::Select;
                            }
                            KeyCode::Char('2') => screen = AppScreen::Downloading,
                            KeyCode::Tab => {
                                if transfer_done {
                                    selected_albums.fill(false);
                                }
                                screen = AppScreen::Select;
                            }
                            _ => {}
                        },
                        Event::Mouse(mouse) => {
                            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    if let Some(next_screen) =
                                        screen_from_header_click(chunks[0], mouse.column, mouse.row)
                                    {
                                        if next_screen == AppScreen::Select && transfer_done {
                                            selected_albums.fill(false);
                                        }
                                        screen = next_screen;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    if let Some(handle) = download_handle.take() {
        handle.abort();
    }

    terminal.show_cursor()?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = Config::load(cli.config.as_deref())?;

    if let Some(output) = cli.output {
        config.download.output_dir = output;
    }

    if let Some(concurrency) = cli.concurrency {
        config.download.concurrency = concurrency.max(1);
    }

    config.validate()?;

    if cli.print_config {
        print_config_summary(&config);
        return Ok(());
    }

    if cli.clean_parts {
        let removed = clean_partial_files(&config.download.output_dir, cli.dry_run, cli.yes)?;
        println!(
            "{} {} PARTIAL FILE{} REMOVED",
            "MSR//".cyan().bold(),
            removed,
            if removed == 1 { "" } else { "S" }
        );
        return Ok(());
    }

    let api = ApiClient::new(&config.api)?;

    if !cli.cli && !cli.list && !cli.all && cli.album.is_none() {
        run_tui(&api, &config).await?;
        return Ok(());
    }

    println!("{}", "MONSTER SIREN RECORDS // DOWNLOADER".cyan().bold());

    if cli.list {
        let albums = api.get_albums().await?;
        for a in albums {
            println!("  {}  {}", a.cid.dimmed(), a.name);
        }
        return Ok(());
    }

    let cli_progress_mode = if cli.no_progress {
        downloader::CliProgressMode::Summary
    } else if cli.plain {
        downloader::CliProgressMode::Plain
    } else {
        downloader::CliProgressMode::Auto
    };

    if let Some(names) = cli.album {
        downloader::download_albums_by_name(&api, &config, &names, cli_progress_mode).await?;
    } else if cli.all {
        downloader::download_all(&api, &config, cli_progress_mode).await?;
    } else {
        anyhow::bail!("no CLI action selected; use --list, --album <name>, or --all");
    }

    println!("\n{}", "MSR// TRANSFER COMPLETE".cyan().bold());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(name: &str) -> models::AlbumBrief {
        models::AlbumBrief {
            cid: name.to_string(),
            name: name.to_string(),
            cover_url: String::new(),
            artistes: Vec::new(),
        }
    }

    #[test]
    fn filtered_album_indices_matches_case_insensitive_names() {
        let albums = vec![album("Summer OST"), album("相变临界"), album("winter")];

        assert_eq!(filtered_album_indices(&albums, "sum"), vec![0]);
        assert_eq!(filtered_album_indices(&albums, "相变"), vec![1]);
        assert_eq!(filtered_album_indices(&albums, ""), vec![0, 1, 2]);
    }

    #[test]
    fn ensure_visible_selection_moves_to_first_match() {
        let visible = vec![2, 4, 6];
        let mut selected = 1;

        ensure_visible_selection(&mut selected, &visible);

        assert_eq!(selected, 2);
    }

    #[test]
    fn move_selection_walks_visible_indices_only() {
        let visible = vec![1, 3, 5];
        let mut selected = 3;

        move_selection(&mut selected, &visible, 1);
        assert_eq!(selected, 5);

        move_selection(&mut selected, &visible, -1);
        assert_eq!(selected, 3);
    }

    #[test]
    fn header_click_maps_to_tabs() {
        let header = Rect::new(0, 0, 100, 3);

        assert_eq!(
            screen_from_header_click(header, 1, 1),
            Some(AppScreen::Select)
        );
        assert_eq!(
            screen_from_header_click(header, 16, 1),
            Some(AppScreen::Downloading)
        );
        assert_eq!(screen_from_header_click(header, 40, 1), None);
        assert_eq!(screen_from_header_click(header, 1, 2), None);
    }

    #[test]
    fn album_mouse_action_distinguishes_focus_and_toggle() {
        let list_area = Rect::new(0, 3, 64, 20);

        assert_eq!(
            album_mouse_action(0, 10, list_area, 2, 4),
            Some(AlbumMouseAction::Toggle(0))
        );
        assert_eq!(
            album_mouse_action(0, 10, list_area, 6, 4),
            Some(AlbumMouseAction::Focus(0))
        );
        assert_eq!(album_mouse_action(0, 10, list_area, 70, 4), None);
    }

    #[test]
    fn transfer_idle_requires_no_active_or_completed_transfer() {
        assert!(is_transfer_idle(false, 0, 0));
        assert!(!is_transfer_idle(false, 1, 0));
        assert!(!is_transfer_idle(false, 0, 1));
        assert!(!is_transfer_idle(true, 0, 0));
    }

    #[test]
    fn collect_partial_files_finds_nested_part_files() {
        let root = std::env::temp_dir().join(format!(
            "msr-downloader-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("keep.txt"), b"keep").unwrap();
        std::fs::write(root.join("track.mp3.part"), b"partial").unwrap();
        std::fs::write(nested.join("cover.jpg.part"), b"partial").unwrap();

        let mut files = Vec::new();
        collect_partial_files(&root, &mut files).unwrap();
        files.sort();

        assert_eq!(files.len(), 2);
        assert!(files.contains(&root.join("track.mp3.part")));
        assert!(files.contains(&nested.join("cover.jpg.part")));

        std::fs::remove_dir_all(root).unwrap();
    }
}
