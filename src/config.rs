use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct ApiConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            timeout: default_timeout(),
        }
    }
}

fn default_base_url() -> String {
    "https://monster-siren.hypergryph.com/api".to_string()
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Deserialize, Clone)]
pub struct IncludeConfig {
    #[serde(default = "default_true")]
    pub lyrics: bool,
    #[serde(default = "default_true")]
    pub covers: bool,
    #[serde(default = "default_true")]
    pub album_info: bool,
    #[serde(default = "default_true")]
    pub metadata: bool,
}

impl Default for IncludeConfig {
    fn default() -> Self {
        Self {
            lyrics: true,
            covers: true,
            album_info: true,
            metadata: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Deserialize, Clone)]
pub struct ConvertConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default = "default_false")]
    pub wav_to_flac: bool,
    #[serde(default = "default_true")]
    pub delete_original: bool,
    #[serde(default = "default_flac_compression")]
    pub flac_compression: u32,
}

impl Default for ConvertConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            wav_to_flac: false,
            delete_original: true,
            flac_compression: 5,
        }
    }
}

fn default_flac_compression() -> u32 {
    5
}

#[derive(Debug, Deserialize, Clone)]
pub struct DownloadConfig {
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub include: IncludeConfig,
    #[serde(default)]
    pub convert: ConvertConfig,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            output_dir: default_output_dir(),
            concurrency: default_concurrency(),
            include: IncludeConfig::default(),
            convert: ConvertConfig::default(),
        }
    }
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("./MSR_Albums")
}

fn default_concurrency() -> usize {
    2
}

#[derive(Debug, Deserialize, Clone)]
pub struct NamingConfig {
    #[serde(default = "default_album_folder")]
    pub album_folder: String,
    #[serde(default = "default_song_file")]
    pub song_file: String,
}

impl Default for NamingConfig {
    fn default() -> Self {
        Self {
            album_folder: default_album_folder(),
            song_file: default_song_file(),
        }
    }
}

fn default_album_folder() -> String {
    "{album_name}".to_string()
}

fn default_song_file() -> String {
    "{song_name}.{ext}".to_string()
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct Config {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub download: DownloadConfig,
    #[serde(default)]
    pub naming: NamingConfig,
}

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let config_path = path.unwrap_or(Path::new("msr.toml"));
        if !config_path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(config_path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(
            config.api.base_url,
            "https://monster-siren.hypergryph.com/api"
        );
        assert_eq!(config.api.timeout, 30);
        assert_eq!(config.download.output_dir, PathBuf::from("./MSR_Albums"));
        assert_eq!(config.download.concurrency, 2);
        assert!(config.download.include.lyrics);
        assert!(config.download.include.covers);
        assert!(config.download.include.metadata);
        assert!(!config.download.convert.enabled);
        assert!(!config.download.convert.wav_to_flac);
    }

    #[test]
    fn test_load_nonexistent_config() {
        let config = Config::load(Some(Path::new("nonexistent.toml"))).unwrap();
        assert_eq!(
            config.api.base_url,
            "https://monster-siren.hypergryph.com/api"
        );
    }

    #[test]
    fn test_parse_config() {
        let toml_str = r#"
[api]
base_url = "https://custom.api.com"
timeout = 60

[download]
output_dir = "./custom"
concurrency = 8

[download.include]
lyrics = false
metadata = true

[download.convert]
enabled = true
wav_to_flac = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.api.base_url, "https://custom.api.com");
        assert_eq!(config.api.timeout, 60);
        assert_eq!(config.download.output_dir, PathBuf::from("./custom"));
        assert_eq!(config.download.concurrency, 8);
        assert!(!config.download.include.lyrics);
        assert!(config.download.include.metadata);
        assert!(config.download.convert.enabled);
        assert!(config.download.convert.wav_to_flac);
    }
}
