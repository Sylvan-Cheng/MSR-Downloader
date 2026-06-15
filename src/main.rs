mod cli;
mod cli_progress;
mod cli_style;
mod tui;

use msr_downloader::api::ApiClient;
use msr_downloader::config::Config;
use msr_downloader::download_session::{self, AlbumDownloadRequest};
use msr_downloader::downloader;
use msr_downloader::models;
use msr_downloader::progress::{
    AlbumDownloadReport, DownloadIssue, DownloadIssueKind, DownloadProgress,
};

use clap::Parser;
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, size as terminal_size, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::task::JoinHandle;
use tui::controller::{
    download_control_action, download_key_action, select_control_action, select_key_action,
    select_row_mouse_decision, SelectRowMouseDecision, TuiAction,
};
use tui::download::{current_transfer_index, download_control_button_at, draw_download_screen};
use tui::input::{album_mouse_action, screen_from_header_click};
use tui::layout::{app_chunks, contains_point, page_step, select_body_chunks};
use tui::overlay::{draw_help_overlay, is_help_close_click};
use tui::select::{
    draw_select_screen, ensure_visible_selection, filtered_album_indices, select_control_button_at,
    visible_select_rows, SelectRow, SelectScreen,
};
use tui::state::{AlbumMouseAction, AppScreen, DownloadScreen, HelpOverlay, TuiState};

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

fn start_tui_download(
    api: &ApiClient,
    config: &Config,
    albums: &[models::AlbumBrief],
    state: &mut TuiState,
    progress: &Arc<Mutex<DownloadProgress>>,
) -> Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>> {
    state.start_queue();
    start_tui_download_from_queue(api, config, albums, state, progress)
}

fn start_tui_download_from_queue(
    api: &ApiClient,
    config: &Config,
    albums: &[models::AlbumBrief],
    state: &mut TuiState,
    progress: &Arc<Mutex<DownloadProgress>>,
) -> Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>> {
    if state.download_queue.is_empty() {
        return None;
    }

    *progress.lock().expect("progress lock poisoned") = DownloadProgress::new("Preparing...", 0);

    let api_clone = api.clone();
    let config_clone = config.clone();
    let progress_clone = progress.clone();
    let queued_albums: Vec<_> = state
        .download_queue
        .iter()
        .enumerate()
        .map(|(queue_idx, &idx)| {
            let options = state
                .download_track_ids
                .get(queue_idx)
                .cloned()
                .flatten()
                .map(downloader::AlbumDownloadOptions::song_ids)
                .unwrap_or_else(downloader::AlbumDownloadOptions::all_tracks);
            AlbumDownloadRequest::new(albums[idx].clone(), options)
        })
        .collect();

    state.screen = AppScreen::Downloading;

    Some(tokio::spawn(async move {
        let session_report = download_session::download_album_session_with_progress(
            &api_clone,
            &config_clone,
            queued_albums,
            Some(progress_clone),
        )
        .await;

        if session_report.has_failures() {
            anyhow::bail!(
                "{} album(s) failed: {}",
                session_report.failures.len(),
                session_report
                    .failures
                    .iter()
                    .map(|failure| failure.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }

        Ok(session_report.albums)
    }))
}

fn apply_tui_action(
    state: &mut TuiState,
    download_handle: &Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>>,
    action: TuiAction,
) -> bool {
    match action {
        TuiAction::None => false,
        TuiAction::Quit => true,
        TuiAction::ConfirmOrQuit => state.confirm_or_quit(download_handle),
        TuiAction::CancelQuit => {
            state.confirm_quit = false;
            false
        }
        TuiAction::OpenHelp => {
            state.open_help();
            false
        }
        TuiAction::CloseHelp => {
            state.close_help();
            false
        }
        TuiAction::SwitchScreen(screen) => {
            state.screen = screen;
            false
        }
        TuiAction::ReturnToAlbums => {
            state.clear_selection_after_done();
            state.screen = AppScreen::Select;
            false
        }
        _ => false,
    }
}

async fn expand_tui_album(
    api: &ApiClient,
    albums: &[models::AlbumBrief],
    state: &mut TuiState,
    album_idx: usize,
    progress: &Arc<Mutex<DownloadProgress>>,
) {
    if let Some(album) = albums.get(album_idx) {
        match api.get_album_detail(&album.cid).await {
            Ok(album_detail) => state.expand_album(album_idx, album_detail),
            Err(e) => {
                if let Ok(mut prog) = progress.lock() {
                    prog.push_issue(DownloadIssue::new(
                        DownloadIssueKind::Album,
                        album.name.as_str(),
                        format!("Album detail error: {}", e),
                    ));
                }
                state.screen = AppScreen::Downloading;
            }
        }
    }
}

fn move_select_row(state: &mut TuiState, rows: &[SelectRow], delta: isize) {
    if rows.is_empty() {
        return;
    }
    let current_row = state
        .selected_track
        .map(|track_idx| SelectRow::Track {
            album_idx: state.selected,
            track_idx,
        })
        .unwrap_or(SelectRow::Album {
            album_idx: state.selected,
        });
    let current = rows.iter().position(|row| *row == current_row).unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        (current + delta as usize).min(rows.len() - 1)
    };
    match rows[next] {
        SelectRow::Album { album_idx } => {
            state.selected = album_idx;
            state.selected_track = None;
        }
        SelectRow::Track {
            album_idx,
            track_idx,
        } => {
            state.selected = album_idx;
            state.selected_track = Some(track_idx);
        }
    }
}

fn focus_select_row(state: &mut TuiState, row: SelectRow) {
    match row {
        SelectRow::Album { album_idx } => {
            state.selected = album_idx;
            state.selected_track = None;
        }
        SelectRow::Track {
            album_idx,
            track_idx,
        } => {
            state.selected = album_idx;
            state.selected_track = Some(track_idx);
        }
    }
}

fn toggle_focused_selection(state: &mut TuiState) {
    if state.selected_track.is_some() {
        state.toggle_focused_track_selection();
    } else {
        state.toggle_album_selection(state.selected);
    }
}

async fn run_tui(api: &ApiClient, config: &Config) -> anyhow::Result<()> {
    let albums = api.get_albums().await.map_err(|e| {
        anyhow::anyhow!(
            "could not load albums from {}; check network, API base URL, or run --print-config: {}",
            config.api.base_url,
            e
        )
    })?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let _terminal_guard = TerminalGuard;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = TuiState::new(albums.len());
    let mut download_handle: Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>> = None;

    let progress = Arc::new(Mutex::new(DownloadProgress::new("", 0)));

    loop {
        if let Some(handle) = download_handle.as_ref() {
            if handle.is_finished() {
                if let Some(handle) = download_handle.take() {
                    match handle.await {
                        Ok(Ok(reports)) => {
                            state.downloaded_names = reports
                                .iter()
                                .map(|report| report.album_name.clone())
                                .collect();
                            state.download_reports = reports;
                        }
                        Ok(Err(e)) => {
                            if let Ok(mut prog) = progress.lock() {
                                prog.push_issue(DownloadIssue::new(
                                    DownloadIssueKind::Task,
                                    "",
                                    format!("Download error: {}", e),
                                ));
                            }
                        }
                        Err(e) => {
                            state.downloaded_names.clear();
                            state.download_reports.clear();
                            if let Ok(mut prog) = progress.lock() {
                                prog.push_issue(DownloadIssue::new(
                                    DownloadIssueKind::Task,
                                    "",
                                    format!("Task error: {}", e),
                                ));
                            }
                        }
                    }
                    state.transfer_done = true;
                }
            }
        }

        match state.screen {
            AppScreen::Select => {
                let visible_indices = filtered_album_indices(&albums, &state.search_query);
                ensure_visible_selection(&mut state.selected, &visible_indices);
                if !matches!(state.expanded_album_idx, Some(idx) if visible_indices.contains(&idx))
                {
                    state.collapse_album();
                }
                let expanded_album = state
                    .expanded_album_idx
                    .and_then(|idx| state.album_detail_cache.get(idx))
                    .and_then(Option::as_ref);
                let visible_rows = visible_select_rows(
                    &albums,
                    &state.search_query,
                    state.expanded_album_idx,
                    expanded_album,
                );

                terminal.draw(|f| {
                    draw_select_screen(
                        f,
                        SelectScreen {
                            albums: &albums,
                            selected: state.selected,
                            selected_track: state.selected_track,
                            selected_albums: &state.selected_albums,
                            album_track_selections: &state.album_track_selections,
                            album_details: &state.album_detail_cache,
                            expanded_album_idx: state.expanded_album_idx,
                            expanded_album,
                            is_downloading: download_handle.is_some(),
                            search_query: &state.search_query,
                            search_active: state.search_active,
                            help_overlay: state.help_overlay,
                        },
                    );
                })?;

                if event::poll(std::time::Duration::from_millis(50))? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            let action = select_key_action(
                                key.code,
                                state.help_overlay,
                                state.search_active,
                                download_handle.is_none(),
                            );
                            match action {
                                TuiAction::CloseHelp => state.close_help(),
                                TuiAction::ClearSearch => state.clear_search(),
                                TuiAction::OpenHelp => state.open_help(),
                                TuiAction::EditSearch => {
                                    state.search_query.pop();
                                }
                                TuiAction::StartSearch => state.search_active = true,
                                TuiAction::PushSearchChar(ch) => state.search_query.push(ch),
                                TuiAction::ConfirmOrQuit => {
                                    if state.confirm_or_quit(&download_handle) {
                                        break;
                                    }
                                }
                                TuiAction::SwitchScreen(screen) => state.screen = screen,
                                TuiAction::MoveSelection(delta) => {
                                    move_select_row(&mut state, &visible_rows, delta);
                                }
                                TuiAction::PageUp => {
                                    if let Ok((width, height)) = terminal_size() {
                                        let chunks = app_chunks(Rect::new(0, 0, width, height));
                                        let body = select_body_chunks(chunks[1]);
                                        move_select_row(
                                            &mut state,
                                            &visible_rows,
                                            -page_step(body[0]),
                                        );
                                    }
                                }
                                TuiAction::PageDown => {
                                    if let Ok((width, height)) = terminal_size() {
                                        let chunks = app_chunks(Rect::new(0, 0, width, height));
                                        let body = select_body_chunks(chunks[1]);
                                        move_select_row(
                                            &mut state,
                                            &visible_rows,
                                            page_step(body[0]),
                                        );
                                    }
                                }
                                TuiAction::MoveHome => {
                                    if let Some(first) = visible_rows.first().copied() {
                                        focus_select_row(&mut state, first);
                                    }
                                }
                                TuiAction::MoveEnd => {
                                    if let Some(last) = visible_rows.last().copied() {
                                        focus_select_row(&mut state, last);
                                    }
                                }
                                TuiAction::ToggleFocusedSelection => {
                                    if !visible_indices.is_empty() {
                                        toggle_focused_selection(&mut state);
                                    }
                                }
                                TuiAction::ToggleAllVisible => {
                                    let all_selected = visible_indices
                                        .iter()
                                        .all(|&idx| state.selected_albums[idx]);
                                    state.set_all_visible_albums(&visible_indices, !all_selected);
                                }
                                TuiAction::ClearSelectionQueue => state.clear_selection_queue(),
                                TuiAction::ApplySearch => state.search_active = false,
                                TuiAction::StartDownload => {
                                    download_handle = start_tui_download(
                                        api, config, &albums, &mut state, &progress,
                                    );
                                }
                                TuiAction::ExpandFocusedAlbum if !visible_indices.is_empty() => {
                                    let album_idx = state.selected;
                                    if state.expanded_album_idx == Some(album_idx)
                                        && state.selected_track.is_none()
                                    {
                                        state.collapse_album();
                                    } else {
                                        expand_tui_album(
                                            api, &albums, &mut state, album_idx, &progress,
                                        )
                                        .await;
                                    }
                                }
                                _ => {}
                            }
                        }
                        Event::Mouse(mouse) => match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    if contains_point(body[0], mouse.column, mouse.row) {
                                        move_select_row(&mut state, &visible_rows, -1);
                                    }
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    if contains_point(body[0], mouse.column, mouse.row) {
                                        move_select_row(&mut state, &visible_rows, 1);
                                    }
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let area = Rect::new(0, 0, width, height);
                                    if state.help_overlay == HelpOverlay::Visible
                                        && is_help_close_click(area, mouse.column, mouse.row)
                                    {
                                        state.close_help();
                                    } else if state.help_overlay == HelpOverlay::Visible {
                                    } else if let Some(next_screen) =
                                        screen_from_header_click(chunks[0], mouse.column, mouse.row)
                                    {
                                        state.screen = next_screen;
                                    } else if let Some(button) = select_control_button_at(
                                        chunks[3],
                                        mouse.column,
                                        mouse.row,
                                        state.search_active,
                                        download_handle.is_some(),
                                        state
                                            .selected_albums
                                            .iter()
                                            .enumerate()
                                            .filter(|(idx, selected_album)| {
                                                **selected_album
                                                    || state
                                                        .album_track_selections
                                                        .get(*idx)
                                                        .and_then(Option::as_ref)
                                                        .is_some_and(|selection| {
                                                            selection
                                                                .iter()
                                                                .any(|&selected| selected)
                                                        })
                                            })
                                            .count(),
                                    ) {
                                        let action = select_control_action(
                                            button,
                                            state.search_active,
                                            download_handle.is_none(),
                                            !visible_indices.is_empty(),
                                        );
                                        match action {
                                            TuiAction::ToggleFocusedSelection => {
                                                toggle_focused_selection(&mut state);
                                            }
                                            TuiAction::ToggleAllVisible => {
                                                let all_selected = visible_indices
                                                    .iter()
                                                    .all(|&idx| state.selected_albums[idx]);
                                                state.set_all_visible_albums(
                                                    &visible_indices,
                                                    !all_selected,
                                                );
                                            }
                                            TuiAction::ClearSearch => state.clear_search(),
                                            TuiAction::ClearSelectionQueue => {
                                                state.clear_selection_queue();
                                                state.clear_search();
                                            }
                                            TuiAction::ExpandFocusedAlbum => {
                                                let album_idx = state.selected;
                                                if state.expanded_album_idx == Some(album_idx)
                                                    && state.selected_track.is_none()
                                                {
                                                    state.collapse_album();
                                                } else {
                                                    expand_tui_album(
                                                        api, &albums, &mut state, album_idx,
                                                        &progress,
                                                    )
                                                    .await;
                                                }
                                            }
                                            TuiAction::StartDownload => {
                                                download_handle = start_tui_download(
                                                    api, config, &albums, &mut state, &progress,
                                                );
                                            }
                                            TuiAction::SwitchScreen(screen) => {
                                                state.screen = screen
                                            }
                                            TuiAction::OpenHelp => state.open_help(),
                                            TuiAction::ConfirmOrQuit => {
                                                if state.confirm_or_quit(&download_handle) {
                                                    break;
                                                }
                                            }
                                            TuiAction::StartSearch => state.search_active = true,
                                            TuiAction::ApplySearch => state.search_active = false,
                                            TuiAction::EditSearch => {
                                                state.search_query.pop();
                                            }
                                            _ => {}
                                        }
                                    } else {
                                        let body = select_body_chunks(chunks[1]);
                                        let selected_row = state
                                            .selected_track
                                            .map(|track_idx| SelectRow::Track {
                                                album_idx: state.selected,
                                                track_idx,
                                            })
                                            .unwrap_or(SelectRow::Album {
                                                album_idx: state.selected,
                                            });
                                        let selected_visible = visible_rows
                                            .iter()
                                            .position(|row| *row == selected_row)
                                            .unwrap_or(0);
                                        let track_rows: Vec<_> = visible_rows
                                            .iter()
                                            .map(|row| matches!(row, SelectRow::Track { .. }))
                                            .collect();
                                        if let Some(action) = album_mouse_action(
                                            selected_visible,
                                            visible_rows.len(),
                                            body[0],
                                            mouse.column,
                                            mouse.row,
                                            &track_rows,
                                        ) {
                                            let index = match action {
                                                AlbumMouseAction::Focus(index)
                                                | AlbumMouseAction::Toggle(index) => index,
                                            };
                                            if let Some(row) = visible_rows.get(index).copied() {
                                                let now = Instant::now();
                                                let decision = select_row_mouse_decision(
                                                    row,
                                                    action,
                                                    download_handle.is_none(),
                                                    state.expanded_album_idx,
                                                    state.last_album_click,
                                                    now,
                                                );
                                                match decision {
                                                    SelectRowMouseDecision::FocusAlbum {
                                                        album_idx,
                                                        record_click,
                                                    } => {
                                                        state.selected = album_idx;
                                                        state.selected_track = None;
                                                        state.last_album_click = record_click
                                                            .then_some((album_idx, now));
                                                    }
                                                    SelectRowMouseDecision::ToggleAlbum {
                                                        album_idx,
                                                    } => {
                                                        state.selected = album_idx;
                                                        state.selected_track = None;
                                                        state.toggle_album_selection(album_idx);
                                                        state.last_album_click = None;
                                                    }
                                                    SelectRowMouseDecision::ExpandAlbum {
                                                        album_idx,
                                                    } => {
                                                        state.selected = album_idx;
                                                        state.selected_track = None;
                                                        expand_tui_album(
                                                            api, &albums, &mut state, album_idx,
                                                            &progress,
                                                        )
                                                        .await;
                                                    }
                                                    SelectRowMouseDecision::CollapseAlbum {
                                                        album_idx,
                                                    } => {
                                                        state.selected = album_idx;
                                                        state.selected_track = None;
                                                        state.collapse_album();
                                                        state.last_album_click = None;
                                                    }
                                                    SelectRowMouseDecision::FocusTrack {
                                                        album_idx,
                                                        track_idx,
                                                    } => {
                                                        state.selected = album_idx;
                                                        state.selected_track = Some(track_idx);
                                                        state.last_album_click = None;
                                                    }
                                                    SelectRowMouseDecision::ToggleTrack {
                                                        album_idx,
                                                        track_idx,
                                                    } => {
                                                        state.selected = album_idx;
                                                        state.selected_track = Some(track_idx);
                                                        state.toggle_focused_track_selection();
                                                        state.last_album_click = None;
                                                    }
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
                    current_transfer_index(&state.download_queue, &albums, &progress)
                {
                    state.download_current = queue_idx;
                    state.active_album_idx = album_idx;
                }

                if let Ok(prog) = progress.lock() {
                    terminal.draw(|f| {
                        draw_download_screen(
                            f,
                            DownloadScreen {
                                albums: &albums,
                                download_queue: &state.download_queue,
                                download_track_ids: &state.download_track_ids,
                                current_album_idx: state.active_album_idx,
                                current: state.download_current + 1,
                                total: state.download_queue.len(),
                                progress: &prog,
                                reports: &state.download_reports,
                                downloaded: &state.downloaded_names,
                                done: state.transfer_done,
                                confirm_quit: state.confirm_quit,
                            },
                        );
                        if state.help_overlay == HelpOverlay::Visible {
                            draw_help_overlay(f, f.area());
                        }
                    })?;
                }

                if event::poll(std::time::Duration::from_millis(80))? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => {
                            let action = download_key_action(
                                key.code,
                                state.help_overlay,
                                state.confirm_quit,
                            );
                            if apply_tui_action(&mut state, &download_handle, action) {
                                break;
                            }
                        }
                        Event::Mouse(mouse) => {
                            if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let area = Rect::new(0, 0, width, height);
                                    if state.help_overlay == HelpOverlay::Visible
                                        && is_help_close_click(area, mouse.column, mouse.row)
                                    {
                                        state.close_help();
                                    } else if state.help_overlay == HelpOverlay::Visible {
                                    } else if let Some(button) = download_control_button_at(
                                        chunks[3],
                                        mouse.column,
                                        mouse.row,
                                        state.confirm_quit,
                                    ) {
                                        let action = download_control_action(button);
                                        if apply_tui_action(&mut state, &download_handle, action) {
                                            break;
                                        }
                                    } else if let Some(next_screen) =
                                        screen_from_header_click(chunks[0], mouse.column, mouse.row)
                                    {
                                        if next_screen == AppScreen::Select {
                                            state.clear_selection_after_done();
                                        }
                                        state.screen = next_screen;
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
    let cli = cli::Cli::parse();

    if cli.init_config {
        let path = cli.config.as_deref().unwrap_or(Path::new("msr.toml"));
        cli::init_config_file(path, cli.yes)?;
        println!("{} CONFIG WRITTEN {}", cli_style::msr(), path.display());
        return Ok(());
    }

    let mut config = Config::load(cli.config.as_deref())?;

    if let Some(output) = cli.output.as_ref() {
        config.download.output_dir = output.clone();
    }

    if let Some(concurrency) = cli.concurrency {
        config.download.concurrency = concurrency.max(1);
    }

    config.validate()?;

    if cli.print_config {
        cli::print_config_summary(&config);
        return Ok(());
    }

    if cli.check_config {
        println!("{} CONFIG OK", cli_style::msr());
        cli::print_config_summary(&config);
        return Ok(());
    }

    if cli.clean_parts {
        match cli::clean_partial_files(&config.download.output_dir, cli.dry_run, cli.yes)? {
            cli::CleanPartsResult::DryRun(count) => println!(
                "{} DRY RUN / {} PARTIAL FILE{} WOULD BE REMOVED",
                cli_style::msr(),
                count,
                if count == 1 { "" } else { "S" }
            ),
            cli::CleanPartsResult::Removed(removed) => println!(
                "{} {} PARTIAL FILE{} REMOVED",
                cli_style::msr(),
                removed,
                if removed == 1 { "" } else { "S" }
            ),
        }
        return Ok(());
    }

    cli::validate_cli_action(&cli)?;

    let api = ApiClient::new(&config.api)?;

    if !cli.cli && !cli.list && !cli.all && cli.album.is_none() && cli.album_id.is_none() {
        println!(
            "{} CONNECTING TO MONSTER SIREN / {}",
            cli_style::msr(),
            config.api.base_url
        );
        run_tui(&api, &config).await?;
        return Ok(());
    }

    println!(
        "{}",
        cli_style::title("MONSTER SIREN RECORDS // DOWNLOADER")
    );

    if cli.list {
        let albums = api.get_albums().await?;
        for a in albums {
            println!("  {}  {}", cli_style::dimmed(&a.cid), a.name);
        }
        return Ok(());
    }

    let cli_progress_mode = if cli.no_progress {
        cli_progress::CliProgressMode::Summary
    } else if cli.plain {
        cli_progress::CliProgressMode::Plain
    } else {
        cli_progress::CliProgressMode::Auto
    };

    let performed_download = if let Some(names) = cli.album {
        cli::download_albums_by_name(
            &api,
            &config,
            &names,
            cli.exact,
            cli.dry_run,
            cli_progress_mode,
            cli.tracks.as_deref(),
        )
        .await?;
        !cli.dry_run
    } else if let Some(ids) = cli.album_id {
        cli::download_albums_by_id(
            &api,
            &config,
            &ids,
            cli.dry_run,
            cli_progress_mode,
            cli.tracks.as_deref(),
        )
        .await?;
        !cli.dry_run
    } else if cli.all {
        cli::download_all(&api, &config, cli_progress_mode, cli.dry_run).await?;
        !cli.dry_run
    } else {
        return Err(cli::no_cli_action_error());
    };

    if performed_download {
        println!("\n{}", cli_style::title("MSR// TRANSFER COMPLETE"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use msr_downloader::models::AlbumBrief;
    use tui::select::move_selection;

    fn album(name: &str) -> models::AlbumBrief {
        models::AlbumBrief {
            cid: name.to_string(),
            name: name.to_string(),
            cover_url: String::new(),
            artists: Vec::new(),
        }
    }

    #[test]
    fn filtered_album_indices_matches_case_insensitive_names() {
        let albums = vec![album("Summer OST"), album("春弦"), album("winter")];

        assert_eq!(filtered_album_indices(&albums, "sum"), vec![0]);
        assert_eq!(filtered_album_indices(&albums, "春弦"), vec![1]);
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
            screen_from_header_click(header, 2, 1),
            Some(AppScreen::Select)
        );
        assert_eq!(
            screen_from_header_click(header, 11, 1),
            Some(AppScreen::Select)
        );
        assert_eq!(
            screen_from_header_click(header, 16, 1),
            Some(AppScreen::Downloading)
        );
        assert_eq!(
            screen_from_header_click(header, 27, 1),
            Some(AppScreen::Downloading)
        );
        assert_eq!(screen_from_header_click(header, 1, 1), None);
        assert_eq!(screen_from_header_click(header, 15, 1), None);
        assert_eq!(screen_from_header_click(header, 41, 1), None);
        assert_eq!(screen_from_header_click(header, 1, 2), None);
    }

    #[test]
    fn album_mouse_action_toggles_clicked_album_row() {
        let list_area = Rect::new(0, 3, 64, 20);

        assert_eq!(
            album_mouse_action(0, 10, list_area, 4, 4, &[false; 10]),
            Some(AlbumMouseAction::Toggle(0))
        );
        assert_eq!(
            album_mouse_action(0, 10, list_area, 8, 4, &[false; 10]),
            Some(AlbumMouseAction::Focus(0))
        );
        assert_eq!(
            album_mouse_action(0, 10, list_area, 2, 4, &[false; 10]),
            Some(AlbumMouseAction::Focus(0))
        );
        let track_rows = [false, true, false];
        assert_eq!(
            album_mouse_action(0, 3, list_area, 5, 5, &track_rows),
            Some(AlbumMouseAction::Toggle(1))
        );
        assert_eq!(
            album_mouse_action(0, 3, list_area, 4, 5, &track_rows),
            Some(AlbumMouseAction::Focus(1))
        );
        assert_eq!(
            album_mouse_action(0, 10, list_area, 70, 4, &[false; 10]),
            None
        );
    }

    #[test]
    fn transfer_idle_requires_no_active_or_completed_transfer() {
        assert!(tui::download::is_transfer_idle(false, 0, 0));
        assert!(!tui::download::is_transfer_idle(false, 1, 0));
        assert!(!tui::download::is_transfer_idle(false, 0, 1));
        assert!(!tui::download::is_transfer_idle(true, 0, 0));
    }

    #[test]
    fn page_step_uses_visible_rows() {
        assert_eq!(page_step(Rect::new(0, 0, 80, 10)), 8);
        assert_eq!(page_step(Rect::new(0, 0, 80, 1)), 1);
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
        std::fs::write(root.join("track.mp3.part.meta"), b"meta").unwrap();
        std::fs::write(nested.join("cover.jpg.part"), b"partial").unwrap();

        let mut files = Vec::new();
        cli::clean::collect_partial_files(&root, &mut files).unwrap();
        files.sort();

        assert_eq!(files.len(), 3);
        assert!(files.contains(&root.join("track.mp3.part")));
        assert!(files.contains(&root.join("track.mp3.part.meta")));
        assert!(files.contains(&nested.join("cover.jpg.part")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn tui_state_new_initializes_correctly() {
        let state = TuiState::new(5);
        assert_eq!(state.screen, AppScreen::Select);
        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_albums, vec![false; 5]);
        assert!(state.search_query.is_empty());
        assert!(!state.search_active);
        assert_eq!(state.help_overlay, HelpOverlay::Hidden);
        assert!(state.downloaded_names.is_empty());
        assert!(state.download_queue.is_empty());
        assert!(state.download_track_ids.is_empty());
        assert_eq!(state.download_current, 0);
        assert!(!state.transfer_done);
        assert_eq!(state.active_album_idx, 0);
        assert!(!state.confirm_quit);
        assert_eq!(state.expanded_album_idx, None);
        assert_eq!(state.album_detail_cache.len(), 5);
        assert!(state.album_detail_cache.iter().all(Option::is_none));
        assert_eq!(state.album_track_selections, vec![None; 5]);
        assert_eq!(state.selected_track, None);
    }

    #[test]
    fn tui_state_clears_search_help_and_selection_state() {
        let mut state = TuiState::new(3);
        state.search_active = true;
        state.search_query = "test".to_string();
        state.selected_albums = vec![true, false, true];
        state.album_track_selections = vec![None, Some(vec![true, false]), Some(vec![false, true])];

        assert_eq!(state.help_overlay, HelpOverlay::Hidden);
        state.open_help();
        assert_eq!(state.help_overlay, HelpOverlay::Visible);
        state.close_help();
        assert_eq!(state.help_overlay, HelpOverlay::Hidden);

        state.clear_search();
        state.clear_selection_queue();

        assert!(!state.search_active);
        assert!(state.search_query.is_empty());
        assert_eq!(state.selected_albums, vec![false, false, false]);
        assert_eq!(state.album_track_selections, vec![None, None, None]);
    }

    #[test]
    fn tui_state_start_queue_builds_from_selected() {
        let mut state = TuiState::new(5);
        state.selected_albums = vec![true, false, true, false, true];

        state.start_queue();

        assert_eq!(state.download_queue, vec![0, 2, 4]);
        assert_eq!(state.download_track_ids, vec![None, None, None]);
        assert_eq!(state.download_current, 0);
        assert!(state.downloaded_names.is_empty());
        assert!(!state.transfer_done);
        assert!(!state.confirm_quit);
        assert_eq!(state.active_album_idx, 0);
    }

    #[test]
    fn tui_state_start_queue_empty_when_nothing_selected() {
        let mut state = TuiState::new(3);
        state.selected_albums = vec![false, false, false];

        state.start_queue();

        assert!(state.download_queue.is_empty());
    }

    #[test]
    fn tui_state_start_queue_uses_partially_selected_song_ids() {
        let mut state = TuiState::new(2);
        state.expand_album(
            1,
            models::AlbumDetail {
                cid: "album".to_string(),
                name: "Album".to_string(),
                intro: String::new(),
                belong: String::new(),
                cover_url: String::new(),
                cover_de_url: String::new(),
                songs: vec![
                    models::SongBrief {
                        cid: "one".to_string(),
                        name: "One".to_string(),
                        artists: Vec::new(),
                    },
                    models::SongBrief {
                        cid: "two".to_string(),
                        name: "Two".to_string(),
                        artists: Vec::new(),
                    },
                ],
            },
        );
        state.album_track_selections[1] = Some(vec![false, true]);

        state.start_queue();

        assert_eq!(state.download_queue, vec![1]);
        assert_eq!(
            state.download_track_ids,
            vec![Some(vec!["two".to_string()])]
        );
    }

    #[test]
    fn tui_state_track_toggles_promote_all_tracks_to_full_album() {
        let mut state = TuiState::new(1);
        state.expand_album(
            0,
            models::AlbumDetail {
                cid: "album".to_string(),
                name: "Album".to_string(),
                intro: String::new(),
                belong: String::new(),
                cover_url: String::new(),
                cover_de_url: String::new(),
                songs: vec![
                    models::SongBrief {
                        cid: "one".to_string(),
                        name: "One".to_string(),
                        artists: Vec::new(),
                    },
                    models::SongBrief {
                        cid: "two".to_string(),
                        name: "Two".to_string(),
                        artists: Vec::new(),
                    },
                ],
            },
        );

        state.selected_track = Some(0);
        state.toggle_focused_track_selection();
        assert_eq!(state.selected_albums, vec![false]);
        assert_eq!(state.album_track_selections[0], Some(vec![true, false]));

        state.selected_track = Some(1);
        state.toggle_focused_track_selection();
        assert_eq!(state.selected_albums, vec![true]);
        assert_eq!(state.album_track_selections[0], None);

        state.start_queue();
        assert_eq!(state.download_queue, vec![0]);
        assert_eq!(state.download_track_ids, vec![None]);
    }

    #[test]
    fn tui_state_clear_selection_after_done_only_when_done() {
        let mut state = TuiState::new(3);
        state.selected_albums = vec![true, true, false];
        state.transfer_done = true;

        state.clear_selection_after_done();

        assert_eq!(state.selected_albums, vec![false, false, false]);
        assert_eq!(state.album_track_selections, vec![None, None, None]);

        state.selected_albums = vec![true, true, false];
        state.transfer_done = false;

        state.clear_selection_after_done();

        assert_eq!(state.selected_albums, vec![true, true, false]);
    }

    #[test]
    fn tui_state_confirm_or_quit_without_transfer_returns_true() {
        let mut state = TuiState::new(3);
        let handle: Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>> = None;

        assert!(state.confirm_or_quit(&handle));
        assert!(!state.confirm_quit);
    }

    #[tokio::test]
    async fn download_control_quit_confirms_active_transfer_before_exit() {
        let mut state = TuiState::new(1);
        state.screen = AppScreen::Select;
        let handle: Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>> =
            Some(tokio::spawn(async { Ok(Vec::new()) }));

        let should_exit = apply_tui_action(&mut state, &handle, TuiAction::ConfirmOrQuit);

        assert!(!should_exit);
        assert!(state.confirm_quit);
        assert_eq!(state.screen, AppScreen::Downloading);

        if let Some(handle) = handle {
            handle.abort();
        }
    }

    #[test]
    fn cli_requires_explicit_action_in_cli_mode() {
        let cli = cli::Cli::try_parse_from(["msr-downloader", "--cli"]).unwrap();
        let error = cli::validate_cli_action(&cli).unwrap_err().to_string();

        assert!(error.contains("no CLI action selected"));
        assert!(error.contains("msr-downloader --cli --list"));
    }

    #[test]
    fn cli_action_flags_parse_and_validate() {
        let cli = cli::Cli::try_parse_from(["msr-downloader", "--cli", "--all"]).unwrap();

        assert!(cli.all);
        assert!(cli::validate_cli_action(&cli).is_ok());

        let dry_run =
            cli::Cli::try_parse_from(["msr-downloader", "--cli", "--album", "test", "--dry-run"])
                .unwrap();
        assert!(dry_run.dry_run);
        assert!(cli::validate_cli_action(&dry_run).is_ok());

        let all_dry_run =
            cli::Cli::try_parse_from(["msr-downloader", "--cli", "--all", "--dry-run"]).unwrap();
        assert!(all_dry_run.all);
        assert!(all_dry_run.dry_run);
        assert!(cli::validate_cli_action(&all_dry_run).is_ok());

        let conflict = cli::Cli::try_parse_from([
            "msr-downloader",
            "--cli",
            "--album",
            "test",
            "--album-id",
            "123",
        ])
        .unwrap();
        let error = cli::validate_cli_action(&conflict).unwrap_err().to_string();

        assert!(error.contains("use either --album or --album-id"));
    }

    #[test]
    fn cli_tracks_parse_and_validate() {
        let cli = cli::Cli::try_parse_from([
            "msr-downloader",
            "--cli",
            "--album-id",
            "123",
            "--tracks",
            "1,3,5-7",
        ])
        .unwrap();

        assert_eq!(cli.tracks.as_deref(), Some("1,3,5-7"));
        assert_eq!(
            cli::parse_track_selection_spec("1,3,5-7").unwrap(),
            vec![1, 3, 5, 6, 7]
        );
        assert!(cli::validate_cli_action(&cli).is_ok());
        assert!(cli::parse_track_selection_spec("3-1").is_err());
        assert!(cli::parse_track_selection_spec("0").is_err());
    }

    #[test]
    fn cli_tracks_rejects_all_downloads() {
        let cli = cli::Cli::try_parse_from(["msr-downloader", "--cli", "--all", "--tracks", "1"])
            .unwrap();
        let error = cli::validate_cli_action(&cli).unwrap_err().to_string();

        assert!(error.contains("--tracks"));
        assert!(error.contains("--all"));
    }

    #[test]
    fn cli_clean_parts_requires_yes() {
        let cli = cli::Cli::try_parse_from(["msr-downloader", "--clean-parts"]).unwrap();
        assert!(cli.clean_parts);
        assert!(!cli.yes);
    }

    #[test]
    fn cli_print_config_flag_parses() {
        let cli = cli::Cli::try_parse_from(["msr-downloader", "--print-config"]).unwrap();
        assert!(cli.print_config);
    }

    #[test]
    fn default_config_toml_is_valid() {
        use std::path::PathBuf;
        let config: Config = toml::from_str(cli::config::default_config_toml()).unwrap();

        assert!(config.validate().is_ok());
        assert_eq!(config.download.output_dir, PathBuf::from("./MSR_Albums"));
        assert!(config.download.convert.enabled);
        assert!(config.download.convert.wav_to_flac);
    }

    #[test]
    fn init_config_file_refuses_existing_without_overwrite() {
        let root = std::env::temp_dir().join(format!(
            "msr-downloader-init-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("msr.toml");
        std::fs::write(&path, "existing").unwrap();

        assert!(cli::init_config_file(&path, false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "existing");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn init_config_file_writes_sample_config() {
        let root = std::env::temp_dir().join(format!(
            "msr-downloader-init-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("nested").join("msr.toml");

        cli::init_config_file(&path, false).unwrap();
        let config: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(config.validate().is_ok());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn download_albums_by_name_errors_when_no_album_matches() {
        use async_trait::async_trait;
        use msr_downloader::api::MusicSource;

        #[derive(Clone, Default)]
        struct MockSource {
            albums: Vec<AlbumBrief>,
        }

        #[async_trait]
        impl MusicSource for MockSource {
            async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>> {
                Ok(self.albums.clone())
            }
            async fn get_album_detail(&self, _cid: &str) -> anyhow::Result<models::AlbumDetail> {
                unimplemented!()
            }
            async fn get_song(&self, _cid: &str) -> anyhow::Result<models::SongDetail> {
                unimplemented!()
            }
            async fn download_file(&self, _url: &str, _dest: &Path) -> anyhow::Result<()> {
                unimplemented!()
            }
            async fn content_length(&self, _url: &str) -> anyhow::Result<Option<u64>> {
                unimplemented!()
            }
            async fn download_file_with_progress(
                &self,
                _url: &str,
                _dest: &Path,
                _on_progress: &mut (dyn FnMut(msr_downloader::file_fetcher::FileProgress) + Send),
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
        }

        let source = MockSource {
            albums: vec![AlbumBrief {
                cid: "a".to_string(),
                name: "Alpha".to_string(),
                cover_url: String::new(),
                artists: Vec::new(),
            }],
        };

        let error = cli::download_albums_by_name(
            &source,
            &Config::default(),
            &["no-match".to_string()],
            false,
            false,
            cli_progress::CliProgressMode::Summary,
            None,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("no albums matched"));
    }
}
