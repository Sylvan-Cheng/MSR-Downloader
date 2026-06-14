use crate::config::Config;
use crate::models::SongDetail;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

pub(crate) fn sanitize(name: &str) -> String {
    let illegal = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    let result: String = name
        .chars()
        .map(|c| if illegal.contains(&c) { ' ' } else { c })
        .collect();
    let sanitized = result.trim().trim_matches('.');
    if sanitized.is_empty() {
        "untitled".to_string()
    } else {
        sanitized.to_string()
    }
}

pub(crate) fn ext_from_url(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let filename = path.rsplit('/').next().unwrap_or(path);
    if filename.contains('.') {
        filename.rsplit('.').next().unwrap_or("bin").to_string()
    } else {
        "bin".to_string()
    }
}

pub(crate) fn safe_join_child(base: &Path, child: &str) -> anyhow::Result<PathBuf> {
    if child.trim().is_empty() {
        anyhow::bail!("output path component cannot be empty");
    }

    let mut components = Path::new(child).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(base.join(child)),
        _ => anyhow::bail!("output path component must be a single file or folder name: {child}"),
    }
}

pub(crate) fn build_song_path(
    config: &Config,
    path: &Path,
    song: &SongDetail,
) -> anyhow::Result<PathBuf> {
    let song_name = sanitize(&song.name);
    let ext = ext_from_url(&song.source_url);

    let filename = config
        .naming
        .song_file
        .replace("{song_name}", &song_name)
        .replace("{ext}", &ext);

    safe_join_child(path, &filename)
}

pub(crate) fn final_song_path(
    config: &Config,
    path: &Path,
    song: &SongDetail,
) -> anyhow::Result<PathBuf> {
    let dest = build_song_path(config, path, song)?;
    Ok(converted_flac_path(config, &dest, song).unwrap_or(dest))
}

pub(crate) fn validate_song_destinations(
    config: &Config,
    album_path: &Path,
    songs: &[(usize, SongDetail)],
) -> anyhow::Result<()> {
    let mut seen_download_paths = HashSet::new();
    let mut seen_final_paths = HashSet::new();
    for (_, song) in songs {
        let download_path = build_song_path(config, album_path, song)?;
        if !seen_download_paths.insert(download_path.clone()) {
            anyhow::bail!(
                "duplicate download path for song {}: {}",
                song.name,
                download_path.display()
            );
        }

        let final_path = final_song_path(config, album_path, song)?;
        if !seen_final_paths.insert(final_path.clone()) {
            anyhow::bail!(
                "duplicate final output path for song {}: {}",
                song.name,
                final_path.display()
            );
        }
    }
    Ok(())
}

fn converted_flac_path(config: &Config, dest: &Path, song: &SongDetail) -> Option<PathBuf> {
    if !config.download.convert.enabled || !config.download.convert.wav_to_flac {
        return None;
    }

    ext_from_url(&song.source_url)
        .eq_ignore_ascii_case("wav")
        .then(|| dest.with_extension("flac"))
}

pub(crate) fn existing_converted_dest(
    config: &Config,
    dest: &Path,
    song: &SongDetail,
) -> Option<PathBuf> {
    converted_flac_path(config, dest, song).filter(|path| file_is_nonempty(path))
}

fn file_is_nonempty(path: &Path) -> bool {
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.len() > 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SongDetail;

    #[test]
    fn test_sanitize() {
        assert_eq!(sanitize("test:file?name"), "test file name");
        assert_eq!(sanitize("test*file|name"), "test file name");
        assert_eq!(sanitize("  test file  "), "test file");
        assert_eq!(sanitize("normal_file.mp3"), "normal_file.mp3");
        assert_eq!(sanitize("???"), "untitled");
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

    #[test]
    fn safe_join_child_rejects_path_escape() {
        let base = Path::new("album");

        assert!(safe_join_child(base, "../song.mp3").is_err());
        assert!(safe_join_child(base, "nested/song.mp3").is_err());
        assert_eq!(
            safe_join_child(base, "song.mp3").unwrap(),
            base.join("song.mp3")
        );
    }

    #[test]
    fn safe_join_child_rejects_empty() {
        assert!(safe_join_child(Path::new("base"), "").is_err());
        assert!(safe_join_child(Path::new("base"), "  ").is_err());
    }

    #[test]
    fn validate_song_destinations_rejects_duplicates() {
        let config = Config::default();
        let songs = vec![(0, song_detail("1", "same")), (1, song_detail("2", "same"))];

        assert!(validate_song_destinations(&config, Path::new("album"), &songs).is_err());
    }

    #[test]
    fn validate_song_destinations_rejects_converted_flac_collision() {
        let mut config = Config::default();
        config.download.convert.enabled = true;
        config.download.convert.wav_to_flac = true;
        let native_flac = SongDetail {
            source_url: "https://example.com/same.flac".to_string(),
            ..song_detail("2", "same")
        };
        let songs = vec![(0, song_detail("1", "same")), (1, native_flac)];

        let error = validate_song_destinations(&config, Path::new("album"), &songs)
            .unwrap_err()
            .to_string();

        assert!(error.contains("duplicate final output path"));
    }

    #[test]
    fn existing_converted_dest_requires_enabled_existing_wav_conversion() {
        let root = std::env::temp_dir().join(format!(
            "msr-downloader-flac-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wav_path = root.join("song.wav");
        let flac_path = root.join("song.flac");
        std::fs::write(&flac_path, b"flac").unwrap();

        let mut config = Config::default();
        config.download.convert.enabled = false;
        let song = song_detail("1", "song");
        assert!(existing_converted_dest(&config, &wav_path, &song).is_none());

        config.download.convert.enabled = true;
        config.download.convert.wav_to_flac = true;
        assert_eq!(
            existing_converted_dest(&config, &wav_path, &song),
            Some(flac_path)
        );

        let mp3_song = SongDetail {
            source_url: "https://example.com/song.mp3".to_string(),
            ..song_detail("2", "song")
        };
        assert!(existing_converted_dest(&config, &wav_path, &mp3_song).is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_converted_dest_ignores_empty_flac() {
        let root = std::env::temp_dir().join(format!(
            "msr-downloader-empty-flac-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let wav_path = root.join("song.wav");
        std::fs::write(root.join("song.flac"), b"").unwrap();

        let mut config = Config::default();
        config.download.convert.enabled = true;
        config.download.convert.wav_to_flac = true;
        assert!(existing_converted_dest(&config, &wav_path, &song_detail("1", "song")).is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn safe_join_child_rejects_dot_dot_components() {
        assert!(safe_join_child(Path::new("base"), "../escape").is_err());
        assert!(safe_join_child(Path::new("base"), "foo/../../escape").is_err());
    }

    #[test]
    fn safe_join_child_rejects_absolute_paths() {
        assert!(safe_join_child(Path::new("base"), "/etc/passwd").is_err());
    }

    #[test]
    fn sanitize_handles_unicode() {
        assert_eq!(sanitize("歌曲名称"), "歌曲名称");
        assert_eq!(sanitize("song:name/test"), "song name test");
    }

    #[test]
    fn sanitize_preserves_dots_in_middle() {
        assert_eq!(sanitize("song.name.mp3"), "song.name.mp3");
        assert_eq!(sanitize(".hidden"), "hidden");
        assert_eq!(sanitize("..."), "untitled");
    }

    #[test]
    fn ext_from_url_handles_complex_paths() {
        assert_eq!(
            ext_from_url("https://cdn.example.com/path/to/file.mp3?token=abc&size=100"),
            "mp3"
        );
        assert_eq!(
            ext_from_url("https://example.com/file.name.with.dots.flac"),
            "flac"
        );
        assert_eq!(ext_from_url("https://example.com/"), "bin");
    }

    #[test]
    fn build_song_path_uses_config_template() {
        let mut config = Config::default();
        config.naming.song_file = "{song_name}.{ext}".to_string();

        let song = song_detail("1", "Test Song");
        let path = build_song_path(&config, Path::new("album"), &song).unwrap();
        assert_eq!(path, Path::new("album").join("Test Song.wav"));
    }

    #[test]
    fn build_song_path_sanitizes_song_name() {
        let config = Config::default();
        let song = song_detail("1", "Test:Song?Name");
        let path = build_song_path(&config, Path::new("album"), &song).unwrap();
        assert!(path.to_string_lossy().contains("Test Song Name"));
    }

    #[test]
    fn validate_song_destinations_accepts_unique_paths() {
        let config = Config::default();
        let songs = vec![
            (0, song_detail("1", "Song A")),
            (1, song_detail("2", "Song B")),
        ];

        assert!(validate_song_destinations(&config, Path::new("album"), &songs).is_ok());
    }

    #[test]
    fn final_song_path_reflects_wav_to_flac_conversion() {
        let mut config = Config::default();
        config.download.convert.enabled = true;
        config.download.convert.wav_to_flac = true;

        assert_eq!(
            final_song_path(&config, Path::new("album"), &song_detail("1", "song")).unwrap(),
            Path::new("album").join("song.flac")
        );
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
