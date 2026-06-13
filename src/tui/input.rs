use crate::tui::layout::contains_point;
use crate::tui::state::{AlbumMouseAction, AppScreen};
use crossterm::event::KeyCode;
use ratatui::layout::Rect;

pub(crate) fn is_key(code: KeyCode, expected: char) -> bool {
    matches!(code, KeyCode::Char(ch) if ch.eq_ignore_ascii_case(&expected))
}

pub(crate) fn screen_from_header_click(header: Rect, x: u16, y: u16) -> Option<AppScreen> {
    if !contains_point(header, x, y) || y != header.y + 1 {
        return None;
    }

    let content_x = header.x + 1;
    let albums_start = content_x;
    let albums_end = albums_start + "ALBUMS [1]".len() as u16;
    let transfer_start = albums_end + 4;
    let transfer_end = transfer_start + "TRANSFER [2]".len() as u16;

    if x >= albums_start && x < albums_end {
        Some(AppScreen::Select)
    } else if x >= transfer_start && x < transfer_end {
        Some(AppScreen::Downloading)
    } else {
        None
    }
}

pub(crate) fn album_mouse_action(
    selected_visible: usize,
    album_count: usize,
    list_area: Rect,
    mouse_x: u16,
    mouse_y: u16,
) -> Option<AlbumMouseAction> {
    if album_count == 0 || !contains_point(list_area, mouse_x, mouse_y) {
        return None;
    }

    let content_left = list_area.x.saturating_add(2);
    let content_top = list_area.y.saturating_add(1);
    let content_bottom = list_area
        .y
        .saturating_add(list_area.height)
        .saturating_sub(1);
    if mouse_y < content_top || mouse_y >= content_bottom || mouse_x < content_left {
        return None;
    }

    let visible_rows = content_bottom.saturating_sub(content_top) as usize;
    if visible_rows == 0 {
        return None;
    }

    let start = selected_visible
        .saturating_add(1)
        .saturating_sub(visible_rows);
    let row = mouse_y.saturating_sub(content_top) as usize;
    let index = start + row;
    if index >= album_count {
        return None;
    }

    if mouse_x <= content_left + 1 {
        Some(AlbumMouseAction::Toggle(index))
    } else {
        Some(AlbumMouseAction::Focus(index))
    }
}
