use crate::api::{FileProgress, MusicSource};
use crate::config::Config;
use crate::fs_util;
use crate::metadata;
use crate::models::{AlbumDetail, SongDetail};
use crate::progress::{DownloadProgress, SongStatus};
use owo_colors::OwoColorize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Semaphore;

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

pub async fn download_album<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    progress_mode: crate::cli_progress::CliProgressMode,
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

    crate::cli_progress::render_cli_progress(&progress, &download, progress_mode).await?;
    download.await??;
    Ok(())
}

pub async fn download_album_with_progress<A: MusicSource>(
    api: &A,
    album: &AlbumDetail,
    config: &Config,
    progress: Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<()> {
    let album_path = create_album_path(config, album)?;

    if config.download.include.album_info {
        save_album_info(&album_path, album)?;
    }

    let cover_data = if config.download.include.covers {
        download_covers(api, &album_path, album, &progress).await?
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

    fs_util::validate_song_destinations(config, &album_path, &song_details)?;

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
        .replace("{album_name}", &fs_util::sanitize(&album.name));
    let path = fs_util::safe_join_child(&config.download.output_dir, &folder_name)?;
    std::fs::create_dir_all(&path)?;
    Ok(path)
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
            let status = if file_progress.resumed {
                SongStatus::Resuming
            } else {
                SongStatus::Getting
            };
            let task = prog.task_mut_or_insert(current_song, song_name, status);
            let previous_bytes = task.bytes_downloaded;
            let previous_update = task.last_update;
            task.bytes_downloaded = file_progress.downloaded;
            task.total_bytes = file_progress.total;
            task.resumed = file_progress.resumed;
            task.resume_from = file_progress.resume_from;
            task.attempt = file_progress.attempt;
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
            prog.task_mut_or_insert(current_song, song_name, status);
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
                counted = !task.is_done();
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
        let artists = if song.artists.is_empty() {
            "Unknown".to_string()
        } else {
            song.artists.join("、")
        };
        content.push_str(&format!("  {} - {}\n", song.name, artists));
    }

    std::fs::write(path.join("album_info.txt"), content)?;
    Ok(())
}

async fn download_covers<A: MusicSource>(
    api: &A,
    path: &Path,
    album: &AlbumDetail,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<Option<Vec<u8>>> {
    let album_name = fs_util::sanitize(&album.name);
    let mut cover_data: Option<Vec<u8>> = None;

    let ext = fs_util::ext_from_url(&album.cover_url);
    let dest = fs_util::safe_join_child(path, &format!("{}_Cover.{}", album_name, ext))?;
    download_optional_file(
        api,
        &album.cover_url,
        &dest,
        format!("Failed to download cover for {}", album.name),
        progress,
    )
    .await;

    if dest.exists() {
        cover_data = Some(std::fs::read(&dest)?);
    }

    let ext = fs_util::ext_from_url(&album.cover_de_url);
    let dest = fs_util::safe_join_child(path, &format!("{}_CoverDe.{}", album_name, ext))?;
    download_optional_file(
        api,
        &album.cover_de_url,
        &dest,
        format!("Failed to download alternate cover for {}", album.name),
        progress,
    )
    .await;

    Ok(cover_data)
}

async fn download_optional_file<A: MusicSource>(
    api: &A,
    url: &str,
    dest: &Path,
    context: String,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) {
    if dest.exists() {
        return;
    }

    if let Err(e) = api.download_file(url, dest).await {
        push_error(progress, format!("{}: {}", context, e));
    }
}

async fn download_song_with_progress<A: MusicSource>(
    api: &A,
    job: SongDownloadJob,
) -> anyhow::Result<()> {
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

    let dest = fs_util::build_song_path(&config, &album_path, &song)?;
    let existing_converted_dest = fs_util::existing_converted_dest(&config, &dest, &song);

    let downloaded = if let Some(final_dest) = existing_converted_dest.as_deref() {
        skip_existing_converted_file(final_dest, &song, current, &progress);
        false
    } else {
        download_audio_file(api, &song, &dest, current, total, &progress).await?
    };

    let lyrics_text = download_lyrics(api, &config, &album_path, &song, &progress).await?;

    let final_dest = match existing_converted_dest {
        Some(path) => path,
        None => convert_if_needed(&config, &dest, &song, &progress)?,
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
        &progress,
    )?;

    if downloaded {
        finish_progress(&progress, current, false, false);
    }

    Ok(())
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

async fn download_audio_file<A: MusicSource>(
    api: &A,
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

    let mut on_progress = |file_progress| {
        update_progress(progress, current, &song_name, file_progress);
    };
    let result = api
        .download_file_with_progress(&song.source_url, dest, &mut on_progress)
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

async fn existing_file_is_complete<A: MusicSource>(
    api: &A,
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

async fn download_lyrics<A: MusicSource>(
    api: &A,
    config: &Config,
    path: &Path,
    song: &SongDetail,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<Option<String>> {
    if !config.download.include.lyrics {
        return Ok(None);
    }

    let lyric_url = match &song.lyric_url {
        Some(url) => url,
        None => return Ok(None),
    };

    let song_name = fs_util::sanitize(&song.name);
    let lyric_ext = fs_util::ext_from_url(lyric_url);
    let lyric_dest = fs_util::safe_join_child(path, &format!("{}.{}", song_name, lyric_ext))?;

    download_optional_file(
        api,
        lyric_url,
        &lyric_dest,
        format!("Failed to download lyrics for {}", song.name),
        progress,
    )
    .await;

    if lyric_dest.exists() {
        match std::fs::read_to_string(&lyric_dest) {
            Ok(text) => Ok(Some(text)),
            Err(e) => {
                let message = format!("Could not read lyrics for {}: {}", song.name, e);
                push_error(progress, message.clone());
                Ok(None)
            }
        }
    } else {
        Ok(None)
    }
}

fn convert_if_needed(
    config: &Config,
    dest: &Path,
    song: &SongDetail,
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
) -> anyhow::Result<PathBuf> {
    if !config.download.convert.enabled || !config.download.convert.wav_to_flac || !dest.exists() {
        return Ok(dest.to_path_buf());
    }

    let ext = fs_util::ext_from_url(&song.source_url);
    if ext.to_lowercase() != "wav" {
        return Ok(dest.to_path_buf());
    }

    let flac_path = dest.with_extension("flac");
    if fs_util::existing_converted_dest(config, dest, song).is_some() {
        return Ok(flac_path);
    }
    if flac_path.exists() {
        let _ = std::fs::remove_file(&flac_path);
    }

    match metadata::convert_wav_to_flac(dest, &flac_path, config.download.convert.flac_compression)
    {
        Ok(_) => {
            if progress.is_none() {
                eprintln!(
                    "  {} {}",
                    "✓".green().bold(),
                    format!("Converted to FLAC: {}", fs_util::sanitize(&song.name)).green()
                );
            }

            if config.download.convert.delete_original {
                std::fs::remove_file(dest)?;
            }
            Ok(flac_path)
        }
        Err(e) => {
            let message = format!("Failed to convert {} to FLAC: {}", song.name, e);
            push_error(progress, message);
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
    progress: &Option<Arc<Mutex<DownloadProgress>>>,
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
        let message = format!("Failed to write metadata for {}: {}", song.name, e);
        push_error(progress, message);
    }

    Ok(())
}

pub async fn download_all<A: MusicSource>(
    api: &A,
    config: &Config,
    progress_mode: crate::cli_progress::CliProgressMode,
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

pub async fn download_albums_by_name<A: MusicSource>(
    api: &A,
    config: &Config,
    names: &[String],
    exact: bool,
    dry_run: bool,
    progress_mode: crate::cli_progress::CliProgressMode,
) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;
    let matched: Vec<_> = albums
        .iter()
        .filter(|a| {
            names.iter().any(|n| {
                if exact {
                    a.name.eq_ignore_ascii_case(n)
                } else {
                    a.name.to_lowercase().contains(&n.to_lowercase())
                }
            })
        })
        .collect();

    if matched.is_empty() {
        anyhow::bail!("no albums matched the given names; use --list to inspect available albums");
    }

    print_matched_albums("MATCHING", &matched);

    if dry_run {
        return Ok(());
    }

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

pub async fn download_albums_by_id<A: MusicSource>(
    api: &A,
    config: &Config,
    ids: &[String],
    dry_run: bool,
    progress_mode: crate::cli_progress::CliProgressMode,
) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;
    let matched: Vec<_> = albums
        .iter()
        .filter(|a| ids.iter().any(|id| a.cid.eq_ignore_ascii_case(id)))
        .collect();

    if matched.is_empty() {
        anyhow::bail!("no albums matched the given CIDs; use --list to inspect available albums");
    }

    print_matched_albums("MATCHING", &matched);

    if dry_run {
        return Ok(());
    }

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

fn print_matched_albums(label: &str, albums: &[&crate::models::AlbumBrief]) {
    println!(
        "{} {} {} ALBUMS",
        "MSR//".cyan().bold(),
        albums.len(),
        label
    );
    for album in albums {
        println!("  {}  {}", album.cid.dimmed(), album.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::MusicSource;
    use crate::models::AlbumBrief;
    use async_trait::async_trait;
    use std::collections::HashSet;

    #[derive(Clone, Default)]
    struct MockSource {
        albums: Vec<AlbumBrief>,
        fail_detail_cids: HashSet<String>,
        detail_calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl MusicSource for MockSource {
        async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>> {
            Ok(self.albums.clone())
        }

        async fn get_album_detail(&self, cid: &str) -> anyhow::Result<AlbumDetail> {
            self.detail_calls.lock().unwrap().push(cid.to_string());
            if self.fail_detail_cids.contains(cid) {
                anyhow::bail!("detail failed for {cid}");
            }

            Ok(album_detail(cid, &format!("Album {cid}")))
        }

        async fn get_song(&self, cid: &str) -> anyhow::Result<SongDetail> {
            Ok(song_detail(cid, &format!("Song {cid}")))
        }

        async fn download_file(&self, _url: &str, _dest: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        async fn content_length(&self, _url: &str) -> anyhow::Result<Option<u64>> {
            Ok(Some(0))
        }

        async fn download_file_with_progress(
            &self,
            _url: &str,
            _dest: &Path,
            _on_progress: &mut (dyn FnMut(FileProgress) + Send),
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn download_albums_by_name_errors_when_no_album_matches() {
        let source = MockSource {
            albums: vec![album_brief("a", "Alpha")],
            ..Default::default()
        };

        let error = download_albums_by_name(
            &source,
            &Config::default(),
            &["missing".to_string()],
            false,
            true,
            crate::cli_progress::CliProgressMode::Summary,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("no albums matched"));
    }

    #[tokio::test]
    async fn download_albums_by_id_dry_run_matches_without_fetching_detail() {
        let source = MockSource {
            albums: vec![album_brief("a", "Alpha")],
            ..Default::default()
        };

        download_albums_by_id(
            &source,
            &Config::default(),
            &["a".to_string()],
            true,
            crate::cli_progress::CliProgressMode::Summary,
        )
        .await
        .unwrap();

        assert!(source.detail_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn download_all_continues_after_album_detail_error() {
        let mut fail_detail_cids = HashSet::new();
        fail_detail_cids.insert("bad".to_string());
        let source = MockSource {
            albums: vec![album_brief("bad", "Bad"), album_brief("ok", "Ok")],
            fail_detail_cids,
            ..Default::default()
        };
        let mut config = Config::default();
        config.download.include.album_info = false;
        config.download.include.covers = false;

        let error = download_all(
            &source,
            &config,
            crate::cli_progress::CliProgressMode::Summary,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("1 album(s) failed"));
        assert_eq!(
            *source.detail_calls.lock().unwrap(),
            vec!["bad".to_string(), "ok".to_string()]
        );
    }

    fn album_brief(cid: &str, name: &str) -> AlbumBrief {
        AlbumBrief {
            cid: cid.to_string(),
            name: name.to_string(),
            cover_url: String::new(),
            artists: Vec::new(),
        }
    }

    fn album_detail(cid: &str, name: &str) -> AlbumDetail {
        AlbumDetail {
            cid: cid.to_string(),
            name: name.to_string(),
            intro: String::new(),
            belong: String::new(),
            cover_url: String::new(),
            cover_de_url: String::new(),
            songs: Vec::new(),
        }
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
