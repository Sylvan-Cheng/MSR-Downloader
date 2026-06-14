use msr_downloader::config::Config;
use std::path::Path;

use crate::cli_style;

pub fn print_config_summary(config: &Config) {
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

pub(crate) fn default_config_toml() -> &'static str {
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
enabled = true
wav_to_flac = true
delete_original = true
flac_compression = 5

[naming]
album_folder = "{album_name}"
song_file = "{song_name}.{ext}"
"#
}

pub fn init_config_file(path: &Path, overwrite: bool) -> anyhow::Result<()> {
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
