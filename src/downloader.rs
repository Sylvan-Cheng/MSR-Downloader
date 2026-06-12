use crate::api::ApiClient;
use crate::config::Config;
use crate::metadata;
use crate::models::{AlbumDetail, SongDetail};
use crossterm::{
    cursor, execute,
    terminal::{Clear, ClearType},
};
use owo_colors::OwoColorize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct SongProgress {
    pub index: usize,
    pub name: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub done: bool,
    pub skipped: bool,
    pub failed: bool,
}

#[derive(Clone)]
pub struct DownloadProgress {
    pub album_name: String,
    pub total_songs: usize,
    pub completed_songs: usize,
    pub tasks: Vec<SongProgress>,
    pub errors: Vec<String>,
}

pub async fn download_album(
    api: &ApiClient,
    album: &AlbumDetail,
    config: &Config,
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

    render_cli_progress(&progress, &download).await?;
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

    let mut handles = Vec::new();

    for (idx, song_brief) in album.songs.iter().enumerate() {
        let song_detail = api.get_song(&song_brief.cid).await?;

        update_progress(&progress, idx + 1, &song_detail.name, 0, 0);

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
                &album_path_clone,
                &song_detail,
                &album_clone,
                &config_clone,
                idx + 1,
                total,
                cover_data_clone.as_deref(),
                progress_clone,
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
    let path = config.download.output_dir.join(folder_name);
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

fn update_progress(
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
    current_song: usize,
    song_name: &str,
    bytes_downloaded: u64,
    total_bytes: u64,
) {
    if let Some(ref p) = progress {
        if let Ok(mut prog) = p.lock() {
            if let Some(task) = prog
                .tasks
                .iter_mut()
                .find(|task| task.index == current_song)
            {
                task.name = song_name.to_string();
                task.bytes_downloaded = bytes_downloaded;
                task.total_bytes = total_bytes;
                return;
            }

            prog.tasks.push(SongProgress {
                index: current_song,
                name: song_name.to_string(),
                bytes_downloaded,
                total_bytes,
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
    let dest = path.join(format!("{}_Cover.{}", album_name, ext));
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
    let dest = path.join(format!("{}_CoverDe.{}", album_name, ext));
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

async fn download_song_with_progress(
    api: &ApiClient,
    path: &Path,
    song: &SongDetail,
    album: &AlbumDetail,
    config: &Config,
    current: usize,
    total: usize,
    cover_data: Option<&[u8]>,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<()> {
    let dest = build_song_path(config, path, song)?;

    let downloaded = download_audio_file(api, song, &dest, current, total, &progress).await?;

    let lyrics_text = download_lyrics(api, config, path, song).await?;

    let final_dest = convert_if_needed(config, &dest, song)?;

    write_metadata_if_needed(
        config,
        &final_dest,
        song,
        album,
        cover_data,
        lyrics_text,
        downloaded,
    )?;

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

    Ok(path.join(&filename))
}

async fn download_audio_file(
    api: &ApiClient,
    song: &SongDetail,
    dest: &Path,
    current: usize,
    total: usize,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<bool> {
    if dest.exists() {
        if existing_file_is_complete(api, song, dest).await? {
            let size = dest.metadata().map(|metadata| metadata.len()).unwrap_or(1);
            update_progress(progress, current, &song.name, size, size);
            finish_progress(progress, current, true, false);
            return Ok(false);
        }

        let _ = tokio::fs::remove_file(dest).await;
    }

    let song_name = song.name.clone();
    update_progress(progress, current, &song_name, 0, 0);

    let result = api
        .download_file_with_progress(&song.source_url, dest, |bytes, total_bytes| {
            update_progress(progress, current, &song_name, bytes, total_bytes);
        })
        .await;

    match result {
        Ok(_) => {
            finish_progress(progress, current, false, false);
            Ok(true)
        }
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

    match api.content_length(&song.source_url).await? {
        Some(remote_size) => Ok(local_size == remote_size),
        None => Ok(false),
    }
}

async fn render_cli_progress(
    progress: &Arc<Mutex<DownloadProgress>>,
    handle: &tokio::task::JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
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
    }
    eprintln!();
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
        "{} {}  {}",
        "MSR//".cyan().bold(),
        progress.album_name.white().bold(),
        progress_line(
            overall,
            progress.completed_songs as u64,
            progress.total_songs as u64,
            "TRACKS"
        )
    ));

    let mut tasks = progress.tasks.clone();
    tasks.sort_by_key(|task| task.index);
    for task in tasks.iter().rev().take(8).rev() {
        let ratio = if task.total_bytes > 0 {
            task.bytes_downloaded as f64 / task.total_bytes as f64
        } else {
            0.0
        };
        let status = if task.failed {
            "ERR".red().bold().to_string()
        } else if task.skipped {
            "SKP".dimmed().bold().to_string()
        } else if task.done {
            "OK ".cyan().bold().to_string()
        } else {
            "GET".white().bold().to_string()
        };
        lines.push(format!(
            "  {} {:>2}/{:<2}  {}  {}",
            status,
            task.index,
            progress.total_songs,
            progress_line(ratio, task.bytes_downloaded, task.total_bytes, "MB"),
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
    let lyric_dest = path.join(format!("{}.{}", song_name, lyric_ext));

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
        Ok(Some(std::fs::read_to_string(&lyric_dest)?))
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
    result.trim().trim_matches('.').to_string()
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

pub async fn download_all(api: &ApiClient, config: &Config) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;
    println!("{} {} ALBUMS", "MSR//".cyan().bold(), albums.len());

    for (i, album_brief) in albums.iter().enumerate() {
        println!(
            "\n{} [{}/{}] {}",
            "ALBUM".cyan().bold(),
            i + 1,
            albums.len(),
            album_brief.name.white().bold()
        );
        let album_detail = api.get_album_detail(&album_brief.cid).await?;
        download_album(api, &album_detail, config).await?;
    }

    Ok(())
}

pub async fn download_albums_by_name(
    api: &ApiClient,
    config: &Config,
    names: &[String],
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
        println!(
            "{} {}",
            "ERR".red().bold(),
            "NO ALBUMS MATCHED THE GIVEN NAMES.".red()
        );
        return Ok(());
    }

    println!(
        "{} {} MATCHING ALBUMS",
        "MSR//".cyan().bold(),
        matched.len()
    );

    for album_brief in matched {
        println!(
            "\n{} {}",
            "ALBUM".cyan().bold(),
            album_brief.name.white().bold()
        );
        let album_detail = api.get_album_detail(&album_brief.cid).await?;
        download_album(api, &album_detail, config).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize() {
        assert_eq!(sanitize("test:file?name"), "test file name");
        assert_eq!(sanitize("test*file|name"), "test file name");
        assert_eq!(sanitize("  test file  "), "test file");
        assert_eq!(sanitize("normal_file.mp3"), "normal_file.mp3");
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
}
