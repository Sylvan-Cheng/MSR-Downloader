use id3::TagLike;
use std::path::Path;

pub fn write_metadata(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    track: u32,
    cover_data: Option<&[u8]>,
    lyrics: Option<&str>,
) -> anyhow::Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "mp3" | "wav" | "aiff" => write_id3(path, title, artist, album, track, cover_data, lyrics),
        "flac" => write_flac_metadata(path, title, artist, album, track, cover_data, lyrics),
        "m4a" | "mp4" | "m4b" => Ok(()),
        _ => Ok(()),
    }
}

fn write_id3(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    track: u32,
    cover_data: Option<&[u8]>,
    lyrics: Option<&str>,
) -> anyhow::Result<()> {
    let mut tag = id3::Tag::read_from_path(path).unwrap_or_else(|_| id3::Tag::new());

    tag.set_title(title);
    tag.set_artist(artist);
    tag.set_album(album);
    tag.set_track(track);

    if let Some(data) = cover_data {
        let picture = id3::frame::Picture {
            mime_type: "image/jpeg".to_string(),
            picture_type: id3::frame::PictureType::CoverFront,
            description: String::new(),
            data: data.to_vec(),
        };
        tag.add_frame(picture);
    }

    if let Some(text) = lyrics {
        tag.add_frame(id3::frame::Lyrics {
            lang: "eng".to_string(),
            description: String::new(),
            text: text.to_string(),
        });
    }

    tag.write_to_path(path, id3::Version::Id3v24)?;
    Ok(())
}

fn write_flac_metadata(
    _path: &Path,
    _title: &str,
    _artist: &str,
    _album: &str,
    _track: u32,
    _cover_data: Option<&[u8]>,
    _lyrics: Option<&str>,
) -> anyhow::Result<()> {
    // FLAC metadata writing would require additional crate like metaflac
    // For now, skip FLAC metadata
    Ok(())
}

pub fn convert_wav_to_flac(
    wav_path: &Path,
    flac_path: &Path,
    compression_level: u32,
) -> anyhow::Result<()> {
    use flacx::{level::Level, Encoder, EncoderConfig};

    let level = match compression_level {
        0 => Level::Level0,
        1 => Level::Level1,
        2 => Level::Level2,
        3 => Level::Level3,
        4 => Level::Level4,
        5 => Level::Level5,
        6 => Level::Level6,
        7 => Level::Level7,
        8 => Level::Level8,
        _ => Level::Level5,
    };

    let encoder = Encoder::new(EncoderConfig::builder().level(level).build());

    encoder.encode_file(wav_path, flac_path)?;
    Ok(())
}

pub fn get_audio_extension(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url);
    let filename = path.rsplit('/').next().unwrap_or(path);
    if filename.contains('.') {
        filename.rsplit('.').next().unwrap_or("mp3").to_lowercase()
    } else {
        "mp3".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_audio_extension() {
        assert_eq!(get_audio_extension("https://example.com/file.mp3"), "mp3");
        assert_eq!(get_audio_extension("https://example.com/file.wav"), "wav");
        assert_eq!(get_audio_extension("https://example.com/file.flac"), "flac");
        assert_eq!(
            get_audio_extension("https://example.com/file.mp3?token=123"),
            "mp3"
        );
        assert_eq!(get_audio_extension("https://example.com/path/noext"), "mp3");
    }

    #[test]
    fn test_write_metadata_unsupported_format() {
        let path = Path::new("test.ogg");
        let result = write_metadata(path, "title", "artist", "album", 1, None, None);
        assert!(result.is_ok());
    }
}
