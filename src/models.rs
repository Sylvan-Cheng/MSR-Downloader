use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct ApiResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: T,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlbumBrief {
    pub cid: String,
    pub name: String,
    #[serde(rename = "coverUrl")]
    pub cover_url: String,
    #[serde(default, rename = "artistes")]
    pub artists: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SongBrief {
    pub cid: String,
    pub name: String,
    #[serde(default, rename = "artistes")]
    pub artists: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlbumDetail {
    pub cid: String,
    pub name: String,
    #[serde(default)]
    pub intro: String,
    #[serde(default)]
    pub belong: String,
    #[serde(rename = "coverUrl")]
    pub cover_url: String,
    #[serde(rename = "coverDeUrl")]
    pub cover_de_url: String,
    #[serde(default)]
    pub songs: Vec<SongBrief>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SongDetail {
    pub cid: String,
    pub name: String,
    #[serde(rename = "albumCid")]
    pub album_cid: String,
    #[serde(rename = "sourceUrl")]
    pub source_url: String,
    #[serde(rename = "lyricUrl")]
    pub lyric_url: Option<String>,
    #[serde(rename = "mvUrl")]
    pub mv_url: Option<String>,
    #[serde(rename = "mvCoverUrl")]
    pub mv_cover_url: Option<String>,
    #[serde(default)]
    pub artists: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_album_brief() {
        let json = r#"{
            "cid": "123",
            "name": "Test Album",
            "coverUrl": "https://example.com/cover.jpg",
            "artistes": ["Artist 1", "Artist 2"]
        }"#;
        let album: AlbumBrief = serde_json::from_str(json).unwrap();
        assert_eq!(album.cid, "123");
        assert_eq!(album.name, "Test Album");
        assert_eq!(album.cover_url, "https://example.com/cover.jpg");
        assert_eq!(album.artists.len(), 2);
    }

    #[test]
    fn test_parse_song_detail() {
        let json = r#"{
            "cid": "456",
            "name": "Test Song",
            "albumCid": "123",
            "sourceUrl": "https://example.com/song.wav",
            "lyricUrl": "https://example.com/lyric.lrc",
            "artists": ["Artist 1"]
        }"#;
        let song: SongDetail = serde_json::from_str(json).unwrap();
        assert_eq!(song.cid, "456");
        assert_eq!(song.name, "Test Song");
        assert_eq!(song.source_url, "https://example.com/song.wav");
        assert!(song.lyric_url.is_some());
        assert!(song.mv_url.is_none());
    }

    #[test]
    fn test_parse_song_detail_optional_fields() {
        let json = r#"{
            "cid": "789",
            "name": "Minimal Song",
            "albumCid": "123",
            "sourceUrl": "https://example.com/song.mp3"
        }"#;
        let song: SongDetail = serde_json::from_str(json).unwrap();
        assert_eq!(song.cid, "789");
        assert!(song.lyric_url.is_none());
        assert!(song.mv_url.is_none());
        assert!(song.artists.is_empty());
    }
}
