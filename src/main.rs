mod api;
mod cli_progress;
mod cli_style;
mod config;
mod downloader;
mod file_fetcher;
mod format;
mod fs_util;
mod metadata;
mod models;
mod progress;
mod tui;

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
use progress::DownloadProgress;
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tui::download::{current_transfer_index, draw_download_screen};
use tui::input::{album_mouse_action, is_key, screen_from_header_click};
use tui::layout::{app_chunks, contains_point, page_step, select_body_chunks};
use tui::overlay::draw_help_overlay;
use tui::select::{
    draw_select_screen, ensure_visible_selection, filtered_album_indices, move_selection,
    selected_visible_position,
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

#[derive(Parser)]
#[command(
    name = "msr-downloader",
    version,
    about = "Monster Siren Music Downloader",
    after_help = "Examples:\n  msr-downloader\n  msr-downloader --init-config\n  msr-downloader --check-config\n  msr-downloader --cli --list\n  msr-downloader --cli --album \"春弦\"\n  msr-downloader --cli --album \"春弦\" --exact --dry-run\n  msr-downloader --cli --album-id 123456\n  msr-downloader --cli --all --dry-run\n  msr-downloader --cli --all --output ./music\n  msr-downloader --clean-parts --dry-run\n  msr-downloader --clean-parts --yes"
)]
struct Cli {
    #[arg(
        short,
        long,
        value_name = "FILE",
        help_heading = "Config",
        help = "Path to msr.toml config file"
    )]
    config: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "DIR",
        help_heading = "Download",
        help = "Override download output directory"
    )]
    output: Option<PathBuf>,
    #[arg(short, long, num_args = 1.., value_name = "NAME", help_heading = "Download", help = "Download albums whose names contain the given text")]
    album: Option<Vec<String>>,
    #[arg(long, num_args = 1.., value_name = "CID", help_heading = "Download", help = "Download albums by exact album CID from --list")]
    album_id: Option<Vec<String>>,
    #[arg(
        long,
        help_heading = "Download",
        help = "Require --album to match album names exactly"
    )]
    exact: bool,
    #[arg(
        short,
        long,
        help_heading = "Download",
        help = "List available albums and exit"
    )]
    list: bool,
    #[arg(
        long,
        help_heading = "Download",
        help = "Download all albums; required for full-library CLI downloads"
    )]
    all: bool,
    #[arg(
        long,
        help_heading = "General",
        help = "Use command-line mode instead of the default TUI"
    )]
    cli: bool,
    #[arg(
        long,
        help_heading = "Output",
        help = "Print periodic line-based progress; no cursor control"
    )]
    plain: bool,
    #[arg(
        long,
        help_heading = "Output",
        help = "Suppress progress updates; print final summaries only"
    )]
    no_progress: bool,
    #[arg(
        long,
        value_name = "N",
        help_heading = "Download",
        help = "Override concurrent track downloads"
    )]
    concurrency: Option<usize>,
    #[arg(
        long,
        help_heading = "Config",
        help = "Print resolved configuration and exit"
    )]
    print_config: bool,
    #[arg(
        long,
        help_heading = "Config",
        help = "Create a sample config file at --config path or msr.toml"
    )]
    init_config: bool,
    #[arg(
        long,
        help_heading = "Config",
        help = "Validate resolved configuration and exit"
    )]
    check_config: bool,
    #[arg(
        long,
        help_heading = "Maintenance",
        help = "Clean .part files from the output directory"
    )]
    clean_parts: bool,
    #[arg(
        long,
        help_heading = "Maintenance",
        help = "Preview cleanup targets or matched downloads without changing files"
    )]
    dry_run: bool,
    #[arg(
        long,
        help_heading = "Maintenance",
        help = "Confirm destructive cleanup actions"
    )]
    yes: bool,
}

enum CleanPartsResult {
    DryRun(usize),
    Removed(usize),
}

fn print_config_summary(config: &Config) {
    println!("{} CONFIG", cli_style::msr());
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
    println!(
        "  convert.delete_original = {}",
        config.download.convert.delete_original
    );
    println!(
        "  convert.flac_compression = {}",
        config.download.convert.flac_compression
    );
    println!("  naming.album_folder = {}", config.naming.album_folder);
    println!("  naming.song_file = {}", config.naming.song_file);
}

fn default_config_toml() -> &'static str {
    r#"[api]
base_url = "https://monster-siren.hypergryph.com/api"
timeout = 30

[download]
output_dir = "./MSR_Albums"
concurrency = 2

[download.include]
lyrics = true
covers = true
album_info = true
metadata = true

[download.convert]
enabled = false
wav_to_flac = false
delete_original = true
flac_compression = 5

[naming]
album_folder = "{album_name}"
song_file = "{song_name}.{ext}"
"#
}

fn init_config_file(path: &Path, overwrite: bool) -> anyhow::Result<()> {
    if path.exists() && !overwrite {
        anyhow::bail!(
            "refusing to overwrite existing config {}; pass --yes to overwrite",
            path.display()
        );
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, default_config_toml())?;
    Ok(())
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

fn clean_partial_files(
    dir: &std::path::Path,
    dry_run: bool,
    yes: bool,
) -> anyhow::Result<CleanPartsResult> {
    let mut partial_files = Vec::new();
    collect_partial_files(dir, &mut partial_files)?;

    println!(
        "{} SCANNED {} / {} PARTIAL FILE{} FOUND",
        cli_style::msr(),
        dir.display(),
        partial_files.len(),
        if partial_files.len() == 1 { "" } else { "S" }
    );

    if dry_run {
        for file in &partial_files {
            println!("  {}", file.display());
        }
        return Ok(CleanPartsResult::DryRun(partial_files.len()));
    }

    if !partial_files.is_empty() && !yes {
        anyhow::bail!("refusing to delete partial files without --yes; use --dry-run to preview");
    }

    for file in &partial_files {
        std::fs::remove_file(file)?;
    }
    Ok(CleanPartsResult::Removed(partial_files.len()))
}

fn no_cli_action_error() -> anyhow::Error {
    anyhow::anyhow!(
        "no CLI action selected.\nTry:\n  msr-downloader --cli --list\n  msr-downloader --cli --album \"春弦\" --dry-run\n  msr-downloader --cli --all"
    )
}

fn validate_cli_action(cli: &Cli) -> anyhow::Result<()> {
    if cli.album.is_some() && cli.album_id.is_some() {
        anyhow::bail!("use either --album or --album-id, not both");
    }

    if cli.cli && !cli.list && !cli.all && cli.album.is_none() && cli.album_id.is_none() {
        return Err(no_cli_action_error());
    }

    Ok(())
}

fn start_tui_download(
    api: &ApiClient,
    config: &Config,
    albums: &[models::AlbumBrief],
    state: &mut TuiState,
    progress: &Arc<Mutex<DownloadProgress>>,
) -> Option<JoinHandle<anyhow::Result<Vec<String>>>> {
    state.start_queue();
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
        .map(|&idx| (idx, albums[idx].clone()))
        .collect();

    state.screen = AppScreen::Downloading;

    Some(tokio::spawn(async move {
        let mut downloaded = Vec::new();
        let mut failures = Vec::new();
        for (_, album) in queued_albums {
            let album_detail = match api_clone.get_album_detail(&album.cid).await {
                Ok(album_detail) => album_detail,
                Err(e) => {
                    let message = format!("Album {} detail error: {}", album.name, e);
                    if let Ok(mut prog) = progress_clone.lock() {
                        prog.errors.push(message.clone());
                    }
                    failures.push(message);
                    continue;
                }
            };

            if let Ok(mut prog) = progress_clone.lock() {
                *prog = DownloadProgress::new(&album_detail.name, album_detail.songs.len());
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
                let message = format!("Album {} download error: {}", album.name, e);
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
    }))
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
    let mut download_handle: Option<JoinHandle<anyhow::Result<Vec<String>>>> = None;

    let progress = Arc::new(Mutex::new(DownloadProgress::new("", 0)));

    loop {
        if let Some(handle) = download_handle.as_ref() {
            if handle.is_finished() {
                if let Some(handle) = download_handle.take() {
                    match handle.await {
                        Ok(Ok(names)) => {
                            state.downloaded_names = names;
                        }
                        Ok(Err(e)) => {
                            if let Ok(mut prog) = progress.lock() {
                                prog.errors.push(format!("Download error: {}", e));
                            }
                        }
                        Err(e) => {
                            state.downloaded_names.clear();
                            if let Ok(mut prog) = progress.lock() {
                                prog.errors.push(format!("Task error: {}", e));
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

                terminal.draw(|f| {
                    draw_select_screen(
                        f,
                        &albums,
                        state.selected,
                        &state.selected_albums,
                        download_handle.is_some(),
                        &state.search_query,
                        state.search_active,
                        state.help_overlay,
                    );
                })?;

                if event::poll(std::time::Duration::from_millis(50))? {
                    match event::read()? {
                        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                            KeyCode::Esc if state.help_overlay == HelpOverlay::Visible => {
                                state.close_help();
                            }
                            KeyCode::Esc => {
                                state.clear_search();
                            }
                            KeyCode::Char('?') => {
                                state.open_help();
                            }
                            KeyCode::Backspace if state.search_active => {
                                state.search_query.pop();
                            }
                            KeyCode::Char('/') if !state.search_active => {
                                state.search_active = true;
                            }
                            KeyCode::Char(ch) if state.search_active => {
                                state.search_query.push(ch);
                            }
                            code if is_key(code, 'q') => {
                                if state.confirm_or_quit(&download_handle) {
                                    break;
                                }
                            }
                            KeyCode::Char('1') => state.screen = AppScreen::Select,
                            KeyCode::Char('2') => state.screen = AppScreen::Downloading,
                            KeyCode::Tab => state.screen = AppScreen::Downloading,
                            KeyCode::Up => {
                                move_selection(&mut state.selected, &visible_indices, -1);
                            }
                            KeyCode::Down => {
                                move_selection(&mut state.selected, &visible_indices, 1);
                            }
                            KeyCode::PageUp => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    move_selection(
                                        &mut state.selected,
                                        &visible_indices,
                                        -page_step(body[0]),
                                    );
                                }
                            }
                            KeyCode::PageDown => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    move_selection(
                                        &mut state.selected,
                                        &visible_indices,
                                        page_step(body[0]),
                                    );
                                }
                            }
                            KeyCode::Home => {
                                if let Some(&first) = visible_indices.first() {
                                    state.selected = first;
                                }
                            }
                            KeyCode::End => {
                                if let Some(&last) = visible_indices.last() {
                                    state.selected = last;
                                }
                            }
                            KeyCode::Char(' ') => {
                                if !visible_indices.is_empty() && download_handle.is_none() {
                                    state.selected_albums[state.selected] =
                                        !state.selected_albums[state.selected];
                                }
                            }
                            code if is_key(code, 'a') && download_handle.is_none() => {
                                let all_selected = visible_indices
                                    .iter()
                                    .all(|&idx| state.selected_albums[idx]);
                                for &idx in &visible_indices {
                                    state.selected_albums[idx] = !all_selected;
                                }
                            }
                            code if is_key(code, 'c') && download_handle.is_none() => {
                                state.selected_albums.fill(false);
                            }
                            KeyCode::Enter if state.search_active => {
                                state.search_active = false;
                            }
                            KeyCode::Enter if download_handle.is_none() => {
                                download_handle =
                                    start_tui_download(api, config, &albums, &mut state, &progress);
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
                                        move_selection(&mut state.selected, &visible_indices, -1);
                                    }
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    let body = select_body_chunks(chunks[1]);
                                    if contains_point(body[0], mouse.column, mouse.row) {
                                        move_selection(&mut state.selected, &visible_indices, 1);
                                    }
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if let Ok((width, height)) = terminal_size() {
                                    let chunks = app_chunks(Rect::new(0, 0, width, height));
                                    if let Some(next_screen) =
                                        screen_from_header_click(chunks[0], mouse.column, mouse.row)
                                    {
                                        state.screen = next_screen;
                                    } else {
                                        let body = select_body_chunks(chunks[1]);
                                        if let Some(action) = album_mouse_action(
                                            selected_visible_position(
                                                &visible_indices,
                                                state.selected,
                                            ),
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
                                                state.selected = album_index;
                                                if matches!(action, AlbumMouseAction::Toggle(_))
                                                    && download_handle.is_none()
                                                {
                                                    state.selected_albums[state.selected] =
                                                        !state.selected_albums[state.selected];
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
                                selected_albums: &state.selected_albums,
                                current_album_idx: state.active_album_idx,
                                current: state.download_current + 1,
                                total: state.download_queue.len(),
                                progress: &prog,
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
                        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                            KeyCode::Esc if state.help_overlay == HelpOverlay::Visible => {
                                state.close_help();
                            }
                            KeyCode::Char('?') => {
                                state.open_help();
                            }
                            code if is_key(code, 'q') => {
                                if state.confirm_or_quit(&download_handle) {
                                    break;
                                }
                            }
                            code if state.confirm_quit && is_key(code, 'y') => break,
                            code if state.confirm_quit
                                && (is_key(code, 'n') || code == KeyCode::Esc) =>
                            {
                                state.confirm_quit = false;
                            }
                            KeyCode::Char('1') => {
                                state.clear_selection_after_done();
                                state.screen = AppScreen::Select;
                            }
                            KeyCode::Char('2') => state.screen = AppScreen::Downloading,
                            KeyCode::Tab => {
                                state.clear_selection_after_done();
                                state.screen = AppScreen::Select;
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
    let cli = Cli::parse();

    if cli.init_config {
        let path = cli.config.as_deref().unwrap_or(Path::new("msr.toml"));
        init_config_file(path, cli.yes)?;
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
        print_config_summary(&config);
        return Ok(());
    }

    if cli.check_config {
        println!("{} CONFIG OK", cli_style::msr());
        print_config_summary(&config);
        return Ok(());
    }

    if cli.clean_parts {
        match clean_partial_files(&config.download.output_dir, cli.dry_run, cli.yes)? {
            CleanPartsResult::DryRun(count) => println!(
                "{} DRY RUN / {} PARTIAL FILE{} WOULD BE REMOVED",
                cli_style::msr(),
                count,
                if count == 1 { "" } else { "S" }
            ),
            CleanPartsResult::Removed(removed) => println!(
                "{} {} PARTIAL FILE{} REMOVED",
                cli_style::msr(),
                removed,
                if removed == 1 { "" } else { "S" }
            ),
        }
        return Ok(());
    }

    validate_cli_action(&cli)?;

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
        downloader::download_albums_by_name(
            &api,
            &config,
            &names,
            cli.exact,
            cli.dry_run,
            cli_progress_mode,
        )
        .await?;
        !cli.dry_run
    } else if let Some(ids) = cli.album_id {
        downloader::download_albums_by_id(&api, &config, &ids, cli.dry_run, cli_progress_mode)
            .await?;
        !cli.dry_run
    } else if cli.all {
        downloader::download_all(&api, &config, cli_progress_mode, cli.dry_run).await?;
        !cli.dry_run
    } else {
        return Err(no_cli_action_error());
    };

    if performed_download {
        println!("\n{}", cli_style::title("MSR// TRANSFER COMPLETE"));
    }
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
        std::fs::write(nested.join("cover.jpg.part"), b"partial").unwrap();

        let mut files = Vec::new();
        collect_partial_files(&root, &mut files).unwrap();
        files.sort();

        assert_eq!(files.len(), 2);
        assert!(files.contains(&root.join("track.mp3.part")));
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
        assert_eq!(state.download_current, 0);
        assert!(!state.transfer_done);
        assert_eq!(state.active_album_idx, 0);
        assert!(!state.confirm_quit);
    }

    #[test]
    fn tui_state_clear_search_resets_search_state() {
        let mut state = TuiState::new(3);
        state.search_active = true;
        state.search_query = "test".to_string();

        state.clear_search();

        assert!(!state.search_active);
        assert!(state.search_query.is_empty());
    }

    #[test]
    fn tui_state_help_overlay_toggles() {
        let mut state = TuiState::new(3);
        assert_eq!(state.help_overlay, HelpOverlay::Hidden);

        state.open_help();
        assert_eq!(state.help_overlay, HelpOverlay::Visible);

        state.close_help();
        assert_eq!(state.help_overlay, HelpOverlay::Hidden);
    }

    #[test]
    fn tui_state_start_queue_builds_from_selected() {
        let mut state = TuiState::new(5);
        state.selected_albums = vec![true, false, true, false, true];

        state.start_queue();

        assert_eq!(state.download_queue, vec![0, 2, 4]);
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
    fn tui_state_clear_selection_after_done_clears_when_done() {
        let mut state = TuiState::new(3);
        state.selected_albums = vec![true, true, false];
        state.transfer_done = true;

        state.clear_selection_after_done();

        assert_eq!(state.selected_albums, vec![false, false, false]);
    }

    #[test]
    fn tui_state_clear_selection_after_done_noop_when_not_done() {
        let mut state = TuiState::new(3);
        state.selected_albums = vec![true, true, false];
        state.transfer_done = false;

        state.clear_selection_after_done();

        assert_eq!(state.selected_albums, vec![true, true, false]);
    }

    #[test]
    fn tui_state_confirm_or_quit_without_transfer_returns_true() {
        let mut state = TuiState::new(3);
        let handle: Option<JoinHandle<anyhow::Result<Vec<String>>>> = None;

        assert!(state.confirm_or_quit(&handle));
        assert!(!state.confirm_quit);
    }

    #[test]
    fn cli_requires_explicit_action_in_cli_mode() {
        let cli = Cli::try_parse_from(["msr-downloader", "--cli"]).unwrap();
        let error = validate_cli_action(&cli).unwrap_err().to_string();

        assert!(error.contains("no CLI action selected"));
        assert!(error.contains("msr-downloader --cli --list"));
    }

    #[test]
    fn cli_all_flag_parses() {
        let cli = Cli::try_parse_from(["msr-downloader", "--cli", "--all"]).unwrap();

        assert!(validate_cli_action(&cli).is_ok());
    }

    #[test]
    fn cli_album_and_album_id_conflict() {
        let cli = Cli::try_parse_from([
            "msr-downloader",
            "--cli",
            "--album",
            "test",
            "--album-id",
            "123",
        ])
        .unwrap();
        let error = validate_cli_action(&cli).unwrap_err().to_string();

        assert!(error.contains("use either --album or --album-id"));
    }

    #[test]
    fn cli_dry_run_flag_parses() {
        let cli = Cli::try_parse_from(["msr-downloader", "--cli", "--album", "test", "--dry-run"])
            .unwrap();

        assert!(cli.dry_run);
        assert!(validate_cli_action(&cli).is_ok());
    }

    #[test]
    fn cli_all_dry_run_flag_parses() {
        let cli = Cli::try_parse_from(["msr-downloader", "--cli", "--all", "--dry-run"]).unwrap();

        assert!(cli.all);
        assert!(cli.dry_run);
        assert!(validate_cli_action(&cli).is_ok());
    }

    #[test]
    fn cli_clean_parts_requires_yes() {
        let cli = Cli::try_parse_from(["msr-downloader", "--clean-parts"]).unwrap();
        assert!(cli.clean_parts);
        assert!(!cli.yes);
    }

    #[test]
    fn cli_print_config_flag_parses() {
        let cli = Cli::try_parse_from(["msr-downloader", "--print-config"]).unwrap();
        assert!(cli.print_config);
    }

    #[test]
    fn default_config_toml_is_valid() {
        let config: Config = toml::from_str(default_config_toml()).unwrap();

        assert!(config.validate().is_ok());
        assert_eq!(config.download.output_dir, PathBuf::from("./MSR_Albums"));
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

        assert!(init_config_file(&path, false).is_err());
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

        init_config_file(&path, false).unwrap();
        let config: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(config.validate().is_ok());

        std::fs::remove_dir_all(root).unwrap();
    }
}
