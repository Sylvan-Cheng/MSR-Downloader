use msr_downloader::models;
use msr_downloader::progress::{AlbumDownloadReport, DownloadProgress};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppScreen {
    Select,
    Downloading,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelpOverlay {
    Hidden,
    Visible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlbumMouseAction {
    Toggle(usize),
}

pub(crate) struct DownloadScreen<'a> {
    pub(crate) albums: &'a [models::AlbumBrief],
    pub(crate) selected_albums: &'a [bool],
    pub(crate) current_album_idx: usize,
    pub(crate) current: usize,
    pub(crate) total: usize,
    pub(crate) progress: &'a DownloadProgress,
    pub(crate) reports: &'a [AlbumDownloadReport],
    pub(crate) downloaded: &'a [String],
    pub(crate) done: bool,
    pub(crate) confirm_quit: bool,
}

pub(crate) struct TuiState {
    pub(crate) screen: AppScreen,
    pub(crate) selected: usize,
    pub(crate) selected_albums: Vec<bool>,
    pub(crate) search_query: String,
    pub(crate) search_active: bool,
    pub(crate) help_overlay: HelpOverlay,
    pub(crate) downloaded_names: Vec<String>,
    pub(crate) download_reports: Vec<AlbumDownloadReport>,
    pub(crate) download_queue: Vec<usize>,
    pub(crate) download_current: usize,
    pub(crate) transfer_done: bool,
    pub(crate) active_album_idx: usize,
    pub(crate) confirm_quit: bool,
}

impl TuiState {
    pub(crate) fn new(album_count: usize) -> Self {
        Self {
            screen: AppScreen::Select,
            selected: 0,
            selected_albums: vec![false; album_count],
            search_query: String::new(),
            search_active: false,
            help_overlay: HelpOverlay::Hidden,
            downloaded_names: Vec::new(),
            download_reports: Vec::new(),
            download_queue: Vec::new(),
            download_current: 0,
            transfer_done: false,
            active_album_idx: 0,
            confirm_quit: false,
        }
    }

    pub(crate) fn transfer_active(
        &self,
        download_handle: &Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>>,
    ) -> bool {
        download_handle.is_some() && !self.transfer_done
    }

    pub(crate) fn clear_search(&mut self) {
        self.search_active = false;
        self.search_query.clear();
    }

    pub(crate) fn open_help(&mut self) {
        self.help_overlay = HelpOverlay::Visible;
    }

    pub(crate) fn close_help(&mut self) {
        self.help_overlay = HelpOverlay::Hidden;
    }

    pub(crate) fn confirm_or_quit(
        &mut self,
        download_handle: &Option<JoinHandle<anyhow::Result<Vec<AlbumDownloadReport>>>>,
    ) -> bool {
        if self.transfer_active(download_handle) {
            self.confirm_quit = true;
            self.screen = AppScreen::Downloading;
            false
        } else {
            true
        }
    }

    pub(crate) fn start_queue(&mut self) {
        self.download_queue = self
            .selected_albums
            .iter()
            .enumerate()
            .filter(|(_, &selected)| selected)
            .map(|(index, _)| index)
            .collect();

        if let Some(&first_album) = self.download_queue.first() {
            self.download_current = 0;
            self.downloaded_names.clear();
            self.download_reports.clear();
            self.transfer_done = false;
            self.confirm_quit = false;
            self.active_album_idx = first_album;
        }
    }

    pub(crate) fn clear_selection_after_done(&mut self) {
        if self.transfer_done {
            self.selected_albums.fill(false);
        }
    }
}
