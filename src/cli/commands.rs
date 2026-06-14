use msr_downloader::api::MusicSource;
use msr_downloader::config::Config;
use msr_downloader::downloader;
use msr_downloader::models;
use msr_downloader::progress::DownloadProgress;

use std::sync::{Arc, Mutex};

use crate::cli_progress;
use crate::cli_style;

pub fn no_cli_action_error() -> anyhow::Error {
    anyhow::anyhow!(
        "no CLI action selected.\nTry:\n  msr-downloader --cli --list\n  msr-downloader --cli --album \"春弦\" --dry-run\n  msr-downloader --cli --all"
    )
}

pub fn validate_cli_action(cli: &super::Cli) -> anyhow::Result<()> {
    if cli.album.is_some() && cli.album_id.is_some() {
        anyhow::bail!("use either --album or --album-id, not both");
    }

    if cli.cli && !cli.list && !cli.all && cli.album.is_none() && cli.album_id.is_none() {
        return Err(no_cli_action_error());
    }

    Ok(())
}

pub async fn download_album<A: MusicSource>(
    api: &A,
    album: &models::AlbumDetail,
    config: &Config,
    progress_mode: cli_progress::CliProgressMode,
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
        async move {
            downloader::download_album_with_progress(&api, &album, &config, Some(progress_clone))
                .await
        }
    });

    cli_progress::render_cli_progress(&progress, &download, progress_mode).await?;
    let report = download.await??;
    cli_progress::print_cli_report_summary(&report);
    if report.has_track_failures() {
        anyhow::bail!(
            "{} track issue(s): {}",
            report.track_failure_count(),
            report
                .issues
                .iter()
                .filter(|issue| issue.kind.is_track_failure())
                .map(|issue| issue.summary())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    Ok(())
}

pub async fn download_all<A: MusicSource>(
    api: &A,
    config: &Config,
    progress_mode: cli_progress::CliProgressMode,
    dry_run: bool,
) -> anyhow::Result<()> {
    let albums = api.get_albums().await?;
    if dry_run {
        let matched: Vec<_> = albums.iter().collect();
        print_matched_albums("AVAILABLE", &matched);
        return Ok(());
    }

    println!("{} {} ALBUMS", cli_style::msr(), albums.len());

    let mut failures = Vec::new();
    for (i, album_brief) in albums.iter().enumerate() {
        println!(
            "\n{} [{}/{}] {}",
            cli_style::title("ALBUM"),
            i + 1,
            albums.len(),
            cli_style::value(&album_brief.name)
        );
        match api.get_album_detail(&album_brief.cid).await {
            Ok(album_detail) => {
                if let Err(e) = download_album(api, &album_detail, config, progress_mode).await {
                    if cli_progress::is_interrupted(&e) {
                        return Err(e);
                    }
                    let message = format!("{}: {}", album_brief.name, e);
                    eprintln!("{} {}", cli_style::error("ERR"), cli_style::error(&message));
                    failures.push(message);
                }
            }
            Err(e) => {
                let message = format!("{}: {}", album_brief.name, e);
                eprintln!("{} {}", cli_style::error("ERR"), cli_style::error(&message));
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
    progress_mode: cli_progress::CliProgressMode,
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
            cli_style::title("ALBUM"),
            cli_style::value(&album_brief.name)
        );
        match api.get_album_detail(&album_brief.cid).await {
            Ok(album_detail) => {
                if let Err(e) = download_album(api, &album_detail, config, progress_mode).await {
                    if cli_progress::is_interrupted(&e) {
                        return Err(e);
                    }
                    let message = format!("{}: {}", album_brief.name, e);
                    eprintln!("{} {}", cli_style::error("ERR"), cli_style::error(&message));
                    failures.push(message);
                }
            }
            Err(e) => {
                let message = format!("{}: {}", album_brief.name, e);
                eprintln!("{} {}", cli_style::error("ERR"), cli_style::error(&message));
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
    progress_mode: cli_progress::CliProgressMode,
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
            cli_style::title("ALBUM"),
            cli_style::value(&album_brief.name)
        );
        match api.get_album_detail(&album_brief.cid).await {
            Ok(album_detail) => {
                if let Err(e) = download_album(api, &album_detail, config, progress_mode).await {
                    if cli_progress::is_interrupted(&e) {
                        return Err(e);
                    }
                    let message = format!("{}: {}", album_brief.name, e);
                    eprintln!("{} {}", cli_style::error("ERR"), cli_style::error(&message));
                    failures.push(message);
                }
            }
            Err(e) => {
                let message = format!("{}: {}", album_brief.name, e);
                eprintln!("{} {}", cli_style::error("ERR"), cli_style::error(&message));
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

pub fn print_matched_albums(label: &str, albums: &[&models::AlbumBrief]) {
    println!("{} {} {} ALBUMS", cli_style::msr(), albums.len(), label);
    for album in albums {
        println!("  {}  {}", cli_style::dimmed(&album.cid), album.name);
    }
}
