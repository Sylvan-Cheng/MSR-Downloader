use msr_downloader::models;
use msr_downloader::progress::{AlbumDownloadReport, DownloadProgress};
use std::time::Instant;
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
    Focus(usize),
    Toggle(usize),
}

pub(crate) struct DownloadScreen<'a> {
    pub(crate) albums: &'a [models::AlbumBrief],
    pub(crate) download_queue: &'a [usize],
    pub(crate) download_track_ids: &'a [Option<Vec<String>>],
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
    pub(crate) download_track_ids: Vec<Option<Vec<String>>>,
    pub(crate) download_current: usize,
    pub(crate) transfer_done: bool,
    pub(crate) active_album_idx: usize,
    pub(crate) confirm_quit: bool,
    pub(crate) expanded_album_idx: Option<usize>,
    pub(crate) album_detail_cache: Vec<Option<models::AlbumDetail>>,
    pub(crate) album_track_selections: Vec<Option<Vec<bool>>>,
    pub(crate) selected_track: Option<usize>,
    pub(crate) last_album_click: Option<(usize, Instant)>,
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
            download_track_ids: Vec::new(),
            download_current: 0,
            transfer_done: false,
            active_album_idx: 0,
            confirm_quit: false,
            expanded_album_idx: None,
            album_detail_cache: vec![None; album_count],
            album_track_selections: vec![None; album_count],
            selected_track: None,
            last_album_click: None,
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

    pub(crate) fn clear_selection_queue(&mut self) {
        self.selected_albums.fill(false);
        self.album_track_selections.fill(None);
    }

    pub(crate) fn start_queue(&mut self) {
        self.download_queue.clear();
        self.download_track_ids.clear();

        for (index, selected_album) in self.selected_albums.iter().copied().enumerate() {
            if selected_album {
                self.download_queue.push(index);
                self.download_track_ids.push(None);
                continue;
            }

            let Some(selection) = self
                .album_track_selections
                .get(index)
                .and_then(Option::as_ref)
            else {
                continue;
            };
            if !selection.iter().any(|&selected| selected) {
                continue;
            }
            let Some(album) = self.album_detail_cache.get(index).and_then(Option::as_ref) else {
                continue;
            };
            let song_ids: Vec<_> = album
                .songs
                .iter()
                .zip(selection.iter())
                .filter(|(_, selected)| **selected)
                .map(|(song, _)| song.cid.clone())
                .collect();
            self.download_queue.push(index);
            self.download_track_ids.push(Some(song_ids));
        }

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
            self.clear_selection_queue();
        }
    }

    pub(crate) fn toggle_album_selection(&mut self, index: usize) {
        if self.selected_albums[index] {
            self.selected_albums[index] = false;
        } else {
            self.selected_albums[index] = true;
            self.album_track_selections[index] = None;
        }
    }

    pub(crate) fn set_all_visible_albums(&mut self, indices: &[usize], selected: bool) {
        for &idx in indices {
            self.selected_albums[idx] = selected;
            if selected {
                self.album_track_selections[idx] = None;
            }
        }
    }

    pub(crate) fn expand_album(&mut self, index: usize, album: models::AlbumDetail) {
        self.selected = index;
        self.expanded_album_idx = Some(index);
        self.selected_track = None;
        if self.album_track_selections[index].is_none() {
            self.album_track_selections[index] = Some(vec![false; album.songs.len()]);
        }
        self.album_detail_cache[index] = Some(album);
        self.screen = AppScreen::Select;
        self.last_album_click = None;
    }

    pub(crate) fn collapse_album(&mut self) {
        self.screen = AppScreen::Select;
        self.expanded_album_idx = None;
        self.selected_track = None;
    }

    pub(crate) fn toggle_focused_track_selection(&mut self) -> Option<()> {
        let album_idx = self.expanded_album_idx?;
        let track_idx = self.selected_track?;
        let track_count = self
            .album_detail_cache
            .get(album_idx)?
            .as_ref()?
            .songs
            .len();
        let selection =
            self.album_track_selections[album_idx].get_or_insert_with(|| vec![false; track_count]);
        if self.selected_albums[album_idx] {
            self.selected_albums[album_idx] = false;
            selection.fill(true);
        }

        let selected = selection.get_mut(track_idx)?;
        *selected = !*selected;

        if selection.iter().all(|&selected| selected) {
            self.selected_albums[album_idx] = true;
            self.album_track_selections[album_idx] = None;
        }
        Some(())
    }
}
