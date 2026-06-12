mod api;
mod config;
mod downloader;
mod metadata;
mod models;

use api::ApiClient;
use clap::Parser;
use config::Config;
use crossterm::{
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
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

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

fn clean_partial_files(dir: &std::path::Path) -> anyhow::Result<usize> {
    if !dir.exists() {
        return Ok(0);
    }

    let mut removed = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            removed += clean_partial_files(&path)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".part"))
        {
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AppScreen {
    Select,
    Downloading,
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    draw_app_header(
        f,
        chunks[0],
        AppScreen::Select,
        "WELCOME TO A WORLD FAMILIARLY UNKNOWN".to_string(),
        COLOR_PRIMARY,
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(64), Constraint::Percentage(36)])
        .split(chunks[1]);

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
            Span::raw(if is_downloading {
                " PROGRESS  "
            } else {
                " PROGRESS  "
            }),
            Span::styled("Q", Style::default().fg(COLOR_INFO)),
            Span::raw(" QUIT"),
        ]),
    );
}

fn draw_download_screen(
    f: &mut ratatui::Frame,
    albums: &[models::AlbumBrief],
    selected_albums: &[bool],
    current_album_idx: usize,
    current: usize,
    total: usize,
    progress: &DownloadProgress,
    downloaded: &[String],
    done: bool,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    let is_idle = !done && total == 0 && progress.total_songs == 0;
    let title_color = if is_idle {
        COLOR_MUTED
    } else if done && progress.failed_count() > 0 {
        COLOR_ERROR
    } else if done {
        COLOR_SUCCESS
    } else {
        COLOR_WARNING
    };
    let title_text = if is_idle {
        "TRANSFER IDLE / NO ACTIVE QUEUE".to_string()
    } else if done {
        if progress.failed_count() > 0 {
            format!(
                "TRANSFER INCOMPLETE / {} OK / {} FAILED",
                progress.ok_count(),
                progress.failed_count()
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
        .filter(|(idx, _)| selected_albums[*idx] || *idx == current_album_idx)
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

        if progress.failed_count() > 0 {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "FAILED TRACKS",
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

    let status_text = if is_idle {
        "ALBUMS [1]  TRANSFER [2]  TAB SWITCH  Q QUIT  /  NO ACTIVE TRANSFER".to_string()
    } else if done {
        if progress.failed_count() > 0 {
            format!(
                "TAB ALBUMS  Q QUIT  /  TRANSFER INCOMPLETE  /  {} OK  {} SKIPPED  {} FAILED",
                progress.ok_count(),
                progress.skipped_count(),
                progress.failed_count()
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
            Span::raw(" QUIT"),
        ]),
    );
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

fn select_album_at_mouse_row(
    selected: usize,
    album_count: usize,
    mouse_x: u16,
    mouse_y: u16,
) -> Option<usize> {
    let (width, height) = terminal_size().ok()?;
    if album_count == 0 || width < 4 || height < 10 {
        return None;
    }

    let body_top = 3u16;
    let body_bottom = height.saturating_sub(6);
    let browse_left = 2u16;
    let browse_right = ((width as f32) * 0.64).floor() as u16;

    if mouse_x < browse_left
        || mouse_x >= browse_right
        || mouse_y <= body_top
        || mouse_y >= body_bottom
    {
        return None;
    }

    let visible_rows = body_bottom.saturating_sub(body_top + 1) as usize;
    if visible_rows == 0 {
        return None;
    }

    let start = selected.saturating_add(1).saturating_sub(visible_rows);
    let row = mouse_y.saturating_sub(body_top + 1) as usize;
    let index = start + row;

    (index < album_count).then_some(index)
}

async fn run_tui(api: &ApiClient, config: &Config) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let albums = api.get_albums().await?;
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
                            downloaded_names.clear();
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
                            KeyCode::Char('q') => break,
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
                                if !visible_indices.is_empty() {
                                    selected_albums[selected] = !selected_albums[selected];
                                }
                            }
                            KeyCode::Char('a') => {
                                let all_selected =
                                    visible_indices.iter().all(|&idx| selected_albums[idx]);
                                for &idx in &visible_indices {
                                    selected_albums[idx] = !all_selected;
                                }
                            }
                            KeyCode::Char('c') if download_handle.is_none() => {
                                for selected in &mut selected_albums {
                                    *selected = false;
                                }
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
                                        for (_, album) in queued_albums {
                                            let album_detail =
                                                api_clone.get_album_detail(&album.cid).await?;
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
                                            .await?;
                                            downloaded.push(album.name);
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
                                move_selection(&mut selected, &visible_indices, -1);
                            }
                            MouseEventKind::ScrollDown => {
                                move_selection(&mut selected, &visible_indices, 1);
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if let Some(index) = select_album_at_mouse_row(
                                    selected_visible_position(&visible_indices, selected),
                                    visible_indices.len(),
                                    mouse.column,
                                    mouse.row,
                                ) {
                                    if let Some(&album_index) = visible_indices.get(index) {
                                        selected = album_index;
                                        if download_handle.is_none() {
                                            selected_albums[selected] = !selected_albums[selected];
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
                            &albums,
                            &selected_albums,
                            active_album_idx,
                            download_current + 1,
                            download_queue.len().max(1),
                            &prog,
                            &downloaded_names,
                            transfer_done,
                        );
                    })?;
                }

                if event::poll(std::time::Duration::from_millis(80))? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Char('1') => {
                                if transfer_done {
                                    for selected in &mut selected_albums {
                                        *selected = false;
                                    }
                                }
                                screen = AppScreen::Select;
                            }
                            KeyCode::Char('2') => screen = AppScreen::Downloading,
                            KeyCode::Tab => {
                                if transfer_done {
                                    for selected in &mut selected_albums {
                                        *selected = false;
                                    }
                                }
                                screen = AppScreen::Select;
                            }
                            _ => {}
                        },
                        Event::Mouse(mouse) => match mouse.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                if mouse.row
                                    >= terminal_size()
                                        .map(|(_, h)| h.saturating_sub(3))
                                        .unwrap_or(0)
                                {
                                    screen = AppScreen::Select;
                                }
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }
    }

    if let Some(handle) = download_handle.take() {
        handle.abort();
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
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

    if cli.print_config {
        print_config_summary(&config);
        return Ok(());
    }

    if cli.clean_parts {
        let removed = clean_partial_files(&config.download.output_dir)?;
        println!(
            "{} {} PARTIAL FILE{} REMOVED",
            "MSR//".cyan().bold(),
            removed,
            if removed == 1 { "" } else { "S" }
        );
        return Ok(());
    }

    let api = ApiClient::new(&config.api)?;

    if !cli.cli && !cli.list && cli.album.is_none() {
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
    } else {
        downloader::download_all(&api, &config, cli_progress_mode).await?;
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
}
