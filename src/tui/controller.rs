use crate::tui::download::DownloadControlButton;
use crate::tui::input::is_key;
use crate::tui::select::{SelectControlButton, SelectRow};
use crate::tui::state::{AlbumMouseAction, AppScreen, HelpOverlay};
use crossterm::event::KeyCode;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TuiAction {
    None,
    Quit,
    ConfirmOrQuit,
    CancelQuit,
    OpenHelp,
    CloseHelp,
    SwitchScreen(AppScreen),
    ReturnToAlbums,
    ClearSearch,
    StartSearch,
    EditSearch,
    PushSearchChar(char),
    ApplySearch,
    MoveSelection(isize),
    MoveHome,
    MoveEnd,
    PageUp,
    PageDown,
    ToggleFocusedSelection,
    ToggleAllVisible,
    ClearSelectionQueue,
    StartDownload,
    ExpandFocusedAlbum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectRowMouseDecision {
    FocusAlbum {
        album_idx: usize,
        record_click: bool,
    },
    ToggleAlbum {
        album_idx: usize,
    },
    ExpandAlbum {
        album_idx: usize,
    },
    CollapseAlbum {
        album_idx: usize,
    },
    FocusTrack {
        album_idx: usize,
        track_idx: usize,
    },
    ToggleTrack {
        album_idx: usize,
        track_idx: usize,
    },
}

pub(crate) fn download_key_action(
    code: KeyCode,
    help_overlay: HelpOverlay,
    confirm_quit: bool,
) -> TuiAction {
    match code {
        KeyCode::Esc if help_overlay == HelpOverlay::Visible => TuiAction::CloseHelp,
        KeyCode::Char('?') => TuiAction::OpenHelp,
        code if is_key(code, 'q') => TuiAction::ConfirmOrQuit,
        code if confirm_quit && is_key(code, 'y') => TuiAction::Quit,
        code if confirm_quit && (is_key(code, 'n') || code == KeyCode::Esc) => {
            TuiAction::CancelQuit
        }
        KeyCode::Char('1') | KeyCode::Tab => TuiAction::ReturnToAlbums,
        KeyCode::Char('2') => TuiAction::SwitchScreen(AppScreen::Downloading),
        _ => TuiAction::None,
    }
}

pub(crate) fn download_control_action(button: DownloadControlButton) -> TuiAction {
    match button {
        DownloadControlButton::Albums => TuiAction::ReturnToAlbums,
        DownloadControlButton::Help => TuiAction::OpenHelp,
        DownloadControlButton::Quit => TuiAction::ConfirmOrQuit,
        DownloadControlButton::Abort => TuiAction::Quit,
        DownloadControlButton::Cancel => TuiAction::CancelQuit,
    }
}

pub(crate) fn select_key_action(
    code: KeyCode,
    help_overlay: HelpOverlay,
    search_active: bool,
    can_download: bool,
) -> TuiAction {
    match code {
        KeyCode::Esc if help_overlay == HelpOverlay::Visible => TuiAction::CloseHelp,
        KeyCode::Esc => TuiAction::ClearSearch,
        KeyCode::Char('?') => TuiAction::OpenHelp,
        KeyCode::Backspace if search_active => TuiAction::EditSearch,
        KeyCode::Char('/') if !search_active => TuiAction::StartSearch,
        KeyCode::Char(ch) if search_active => TuiAction::PushSearchChar(ch),
        code if is_key(code, 'q') => TuiAction::ConfirmOrQuit,
        KeyCode::Char('1') => TuiAction::SwitchScreen(AppScreen::Select),
        KeyCode::Char('2') | KeyCode::Tab => TuiAction::SwitchScreen(AppScreen::Downloading),
        KeyCode::Up => TuiAction::MoveSelection(-1),
        KeyCode::Down => TuiAction::MoveSelection(1),
        KeyCode::PageUp => TuiAction::PageUp,
        KeyCode::PageDown => TuiAction::PageDown,
        KeyCode::Home => TuiAction::MoveHome,
        KeyCode::End => TuiAction::MoveEnd,
        KeyCode::Char(' ') if can_download => TuiAction::ToggleFocusedSelection,
        code if is_key(code, 'a') && can_download => TuiAction::ToggleAllVisible,
        code if is_key(code, 'c') && can_download => TuiAction::ClearSelectionQueue,
        KeyCode::Enter if search_active => TuiAction::ApplySearch,
        code if is_key(code, 'd') && can_download => TuiAction::StartDownload,
        KeyCode::Enter if can_download => TuiAction::ExpandFocusedAlbum,
        KeyCode::Enter => TuiAction::None,
        _ => TuiAction::None,
    }
}

pub(crate) fn select_control_action(
    button: SelectControlButton,
    search_active: bool,
    can_download: bool,
    has_visible_albums: bool,
) -> TuiAction {
    match button {
        SelectControlButton::Toggle if can_download && has_visible_albums => {
            TuiAction::ToggleFocusedSelection
        }
        SelectControlButton::SelectAll if can_download => TuiAction::ToggleAllVisible,
        SelectControlButton::Clear if search_active => TuiAction::ClearSearch,
        SelectControlButton::Clear if can_download => TuiAction::ClearSelectionQueue,
        SelectControlButton::Expand if can_download && has_visible_albums => {
            TuiAction::ExpandFocusedAlbum
        }
        SelectControlButton::Download if can_download => TuiAction::StartDownload,
        SelectControlButton::Switch => TuiAction::SwitchScreen(AppScreen::Downloading),
        SelectControlButton::Help => TuiAction::OpenHelp,
        SelectControlButton::Quit => TuiAction::ConfirmOrQuit,
        SelectControlButton::Search => TuiAction::StartSearch,
        SelectControlButton::Apply => TuiAction::ApplySearch,
        SelectControlButton::Edit => TuiAction::EditSearch,
        SelectControlButton::Move => TuiAction::None,
        _ => TuiAction::None,
    }
}

pub(crate) fn select_row_mouse_decision(
    row: SelectRow,
    mouse_action: AlbumMouseAction,
    can_change_selection: bool,
    expanded_album_idx: Option<usize>,
    last_album_click: Option<(usize, Instant)>,
    now: Instant,
) -> SelectRowMouseDecision {
    match row {
        SelectRow::Album { album_idx } => match mouse_action {
            AlbumMouseAction::Toggle(_) if can_change_selection => {
                SelectRowMouseDecision::ToggleAlbum { album_idx }
            }
            AlbumMouseAction::Toggle(_) => SelectRowMouseDecision::FocusAlbum {
                album_idx,
                record_click: false,
            },
            AlbumMouseAction::Focus(_) => {
                let double_clicked = matches!(
                    last_album_click,
                    Some((last_index, last_time))
                        if last_index == album_idx
                            && now.duration_since(last_time) <= Duration::from_millis(400)
                );
                if double_clicked && expanded_album_idx == Some(album_idx) {
                    SelectRowMouseDecision::CollapseAlbum { album_idx }
                } else if double_clicked {
                    SelectRowMouseDecision::ExpandAlbum { album_idx }
                } else {
                    SelectRowMouseDecision::FocusAlbum {
                        album_idx,
                        record_click: true,
                    }
                }
            }
        },
        SelectRow::Track {
            album_idx,
            track_idx,
        } => match mouse_action {
            AlbumMouseAction::Toggle(_) if can_change_selection => {
                SelectRowMouseDecision::ToggleTrack {
                    album_idx,
                    track_idx,
                }
            }
            _ => SelectRowMouseDecision::FocusTrack {
                album_idx,
                track_idx,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_key_action_maps_confirmation_keys() {
        assert_eq!(
            download_key_action(KeyCode::Char('q'), HelpOverlay::Hidden, false),
            TuiAction::ConfirmOrQuit
        );
        assert_eq!(
            download_key_action(KeyCode::Char('y'), HelpOverlay::Hidden, true),
            TuiAction::Quit
        );
        assert_eq!(
            download_key_action(KeyCode::Esc, HelpOverlay::Hidden, true),
            TuiAction::CancelQuit
        );
    }

    #[test]
    fn download_control_action_maps_buttons() {
        assert_eq!(
            download_control_action(DownloadControlButton::Albums),
            TuiAction::ReturnToAlbums
        );
        assert_eq!(
            download_control_action(DownloadControlButton::Abort),
            TuiAction::Quit
        );
    }

    #[test]
    fn select_key_action_maps_search_and_download_keys() {
        assert_eq!(
            select_key_action(KeyCode::Char('/'), HelpOverlay::Hidden, false, true),
            TuiAction::StartSearch
        );
        assert_eq!(
            select_key_action(KeyCode::Char('x'), HelpOverlay::Hidden, true, true),
            TuiAction::PushSearchChar('x')
        );
        assert_eq!(
            select_key_action(KeyCode::Char('d'), HelpOverlay::Hidden, false, true),
            TuiAction::StartDownload
        );
    }

    #[test]
    fn select_row_mouse_decision_detects_album_double_click() {
        let now = Instant::now();
        let decision = select_row_mouse_decision(
            SelectRow::Album { album_idx: 2 },
            AlbumMouseAction::Focus(0),
            true,
            None,
            Some((2, now - Duration::from_millis(200))),
            now,
        );

        assert_eq!(
            decision,
            SelectRowMouseDecision::ExpandAlbum { album_idx: 2 }
        );
    }

    #[test]
    fn select_row_mouse_decision_maps_track_checkbox_toggle() {
        let decision = select_row_mouse_decision(
            SelectRow::Track {
                album_idx: 1,
                track_idx: 3,
            },
            AlbumMouseAction::Toggle(0),
            true,
            Some(1),
            None,
            Instant::now(),
        );

        assert_eq!(
            decision,
            SelectRowMouseDecision::ToggleTrack {
                album_idx: 1,
                track_idx: 3
            }
        );
    }
}
