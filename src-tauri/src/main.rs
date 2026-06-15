#![cfg_attr(windows, windows_subsystem = "windows")]

use msr_downloader::api::ApiClient;
use msr_downloader::config::Config;
use msr_downloader::download_session::{self, AlbumDownloadRequest};
use msr_downloader::downloader::DownloadCancellation;
use msr_downloader::models::{AlbumBrief, AlbumDetail};
use msr_downloader::progress::{DownloadEvent, EventSink};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio::task::JoinHandle;

struct AppState {
    api: ApiClient,
    config: Config,
    active_download: Arc<Mutex<Option<JoinHandle<()>>>>,
    active_cancellation: Arc<Mutex<Option<DownloadCancellation>>>,
}

impl AppState {
    fn load() -> Result<Self, String> {
        let config = Config::load(Some(Path::new("msr.toml"))).map_err(|e| e.to_string())?;
        let api = ApiClient::new(&config.api).map_err(|e| e.to_string())?;
        Ok(Self {
            api,
            config,
            active_download: Arc::new(Mutex::new(None)),
            active_cancellation: Arc::new(Mutex::new(None)),
        })
    }
}

#[derive(Clone)]
struct TauriEventSink {
    app: AppHandle,
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: DownloadEvent) {
        let _ = self.app.emit("download-event", event);
    }
}

#[tauri::command]
async fn list_albums(state: tauri::State<'_, AppState>) -> Result<Vec<AlbumBrief>, String> {
    state.api.get_albums().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_album_detail(
    state: tauri::State<'_, AppState>,
    cid: String,
) -> Result<AlbumDetail, String> {
    state
        .api
        .get_album_detail(&cid)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn download_album(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    cid: String,
) -> Result<(), String> {
    let mut active_download = state
        .active_download
        .lock()
        .map_err(|_| "download state lock poisoned".to_string())?;
    if active_download
        .as_ref()
        .is_some_and(|handle| !handle.is_finished())
    {
        return Err("a download is already running".to_string());
    }

    let api = state.api.clone();
    let config = state.config.clone();
    let active_download_state = state.active_download.clone();
    let active_cancellation_state = state.active_cancellation.clone();
    let cancellation = DownloadCancellation::new();
    *state
        .active_cancellation
        .lock()
        .map_err(|_| "download cancellation lock poisoned".to_string())? =
        Some(cancellation.clone());
    let sink = TauriEventSink { app };
    let request = AlbumDownloadRequest::all_tracks(AlbumBrief {
        cid: cid.clone(),
        name: cid,
        cover_url: String::new(),
        artists: Vec::new(),
    });

    *active_download = Some(tokio::spawn(async move {
        let session_report = download_session::download_album_session_with_events_cancelable(
            &api,
            &config,
            vec![request],
            sink.clone(),
            cancellation,
        )
        .await;
        if session_report.has_failures() {
            sink.emit(DownloadEvent::AlbumFailed {
                error: session_report.failure_message(),
            });
        }
        if let Ok(mut active_download) = active_download_state.lock() {
            active_download.take();
        }
        if let Ok(mut active_cancellation) = active_cancellation_state.lock() {
            active_cancellation.take();
        }
    }));

    Ok(())
}

#[tauri::command]
fn cancel_download(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let cancellation = state
        .active_cancellation
        .lock()
        .map_err(|_| "download cancellation lock poisoned".to_string())?
        .clone();
    if let Some(cancellation) = cancellation {
        cancellation.cancel();
        Ok(())
    } else {
        Err("no active download".to_string())
    }
}

fn main() {
    let state = AppState::load().expect("failed to initialize GUI state");
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_albums,
            get_album_detail,
            download_album,
            cancel_download
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tauri application");
}
