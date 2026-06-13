use crate::config::ApiConfig;
use crate::file_fetcher;
pub use crate::file_fetcher::FileProgress;
use crate::models::{AlbumBrief, AlbumDetail, ApiResponse, SongDetail};
use anyhow::Context;
use async_trait::async_trait;
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::path::Path;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    base_url: String,
}

#[async_trait]
pub trait MusicSource: Clone + Send + Sync + 'static {
    async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>>;
    async fn get_album_detail(&self, cid: &str) -> anyhow::Result<AlbumDetail>;
    async fn get_song(&self, cid: &str) -> anyhow::Result<SongDetail>;
    async fn download_file(&self, url: &str, dest: &Path) -> anyhow::Result<()>;
    async fn content_length(&self, url: &str) -> anyhow::Result<Option<u64>>;
    async fn download_file_with_progress(
        &self,
        url: &str,
        dest: &Path,
        on_progress: &mut (dyn FnMut(FileProgress) + Send),
    ) -> anyhow::Result<()>;
}

#[async_trait]
impl MusicSource for ApiClient {
    async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>> {
        ApiClient::get_albums(self).await
    }

    async fn get_album_detail(&self, cid: &str) -> anyhow::Result<AlbumDetail> {
        ApiClient::get_album_detail(self, cid).await
    }

    async fn get_song(&self, cid: &str) -> anyhow::Result<SongDetail> {
        ApiClient::get_song(self, cid).await
    }

    async fn download_file(&self, url: &str, dest: &Path) -> anyhow::Result<()> {
        ApiClient::download_file(self, url, dest).await
    }

    async fn content_length(&self, url: &str) -> anyhow::Result<Option<u64>> {
        ApiClient::content_length(self, url).await
    }

    async fn download_file_with_progress(
        &self,
        url: &str,
        dest: &Path,
        on_progress: &mut (dyn FnMut(FileProgress) + Send),
    ) -> anyhow::Result<()> {
        ApiClient::download_file_with_progress(self, url, dest, on_progress).await
    }
}

impl ApiClient {
    pub fn new(config: &ApiConfig) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout))
            .build()?;

        Ok(Self {
            client,
            base_url: config.base_url.clone(),
        })
    }

    pub async fn get_albums(&self) -> anyhow::Result<Vec<AlbumBrief>> {
        self.fetch_api("albums").await
    }

    pub async fn get_album_detail(&self, cid: &str) -> anyhow::Result<AlbumDetail> {
        self.fetch_api(&format!("album/{cid}/detail")).await
    }

    pub async fn get_song(&self, cid: &str) -> anyhow::Result<SongDetail> {
        self.fetch_api(&format!("song/{cid}")).await
    }

    async fn fetch_api<T>(&self, path: &str) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}/{}", self.base_url, path);
        let resp: ApiResponse<T> = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to request {url}"))?
            .error_for_status()
            .with_context(|| format!("request failed: {url}"))?
            .json()
            .await
            .with_context(|| format!("failed to parse response from {url}"))?;

        if resp.code != 0 {
            anyhow::bail!("API error: {}", resp.msg);
        }

        Ok(resp.data)
    }

    pub async fn download_file(&self, url: &str, dest: &Path) -> anyhow::Result<()> {
        file_fetcher::download_file(&self.client, url, dest).await
    }

    pub async fn content_length(&self, url: &str) -> anyhow::Result<Option<u64>> {
        file_fetcher::content_length(&self.client, url).await
    }

    pub async fn download_file_with_progress<F>(
        &self,
        url: &str,
        dest: &Path,
        on_progress: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(FileProgress),
    {
        file_fetcher::download_file_with_progress(&self.client, url, dest, on_progress).await
    }
}
