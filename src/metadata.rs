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
            mime_type: image_mime_type(data).to_string(),
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

fn image_mime_type(data: &[u8]) -> &'static str {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

fn write_flac_metadata(
    path: &Path,
    _title: &str,
    _artist: &str,
    _album: &str,
    _track: u32,
    _cover_data: Option<&[u8]>,
    _lyrics: Option<&str>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "FLAC metadata writing is not supported yet for {}; audio file was kept unchanged",
        path.display()
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_metadata_unsupported_format() {
        let path = Path::new("test.ogg");
        let result = write_metadata(path, "title", "artist", "album", 1, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_metadata_flac_reports_unsupported() {
        let path = Path::new("test.flac");
        let error = write_metadata(path, "title", "artist", "album", 1, None, None)
            .unwrap_err()
            .to_string();

        assert!(error.contains("FLAC metadata writing is not supported yet"));
    }

    #[test]
    fn test_image_mime_type_detects_common_formats() {
        assert_eq!(image_mime_type(&[0xFF, 0xD8, 0xFF, 0x00]), "image/jpeg");
        assert_eq!(image_mime_type(b"\x89PNG\r\n\x1a\nrest"), "image/png");
        assert_eq!(image_mime_type(b"RIFFxxxxWEBPrest"), "image/webp");
        assert_eq!(image_mime_type(b"unknown"), "image/jpeg");
    }
}
