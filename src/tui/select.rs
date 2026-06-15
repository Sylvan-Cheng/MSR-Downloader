use crate::tui::chrome::{create_block, draw_app_header, draw_controls_bar, draw_status_bar};
use crate::tui::layout::{app_chunks, select_body_chunks};
use crate::tui::overlay::draw_help_overlay;
use crate::tui::state::{AppScreen, HelpOverlay};
use crate::tui::theme::{COLOR_MUTED, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_SUCCESS};
use msr_downloader::models;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};

const SELECT_SEARCH_CONTROLS: &[(&str, &str)] = &[("Apply", ""), ("Help", "")];
const SELECT_TRANSFER_CONTROLS: &[(&str, &str)] = &[("Search", ""), ("Help", "")];
const SELECT_EMPTY_CONTROLS: &[(&str, &str)] = &[("Search", ""), ("Help", "")];
const SELECT_READY_CONTROLS: &[(&str, &str)] = &[
    ("Download", ""),
    ("Clear", ""),
    ("Search", ""),
    ("Help", ""),
];
const SELECT_SEARCH_PROMPT: &str =
    "Search mode: type to filter | Enter apply | Esc clear | q quit | ";
const SELECT_IDLE_PROMPT: &str = "Click [ ] select | Enter expand | Space toggle | q quit | ";
const SELECT_READY_PROMPT: &str = "Click [ ] select | d download | c clear | q quit | ";
const SELECT_TRANSFER_PROMPT: &str = "Transfer active | Tab transfer | q quit | ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectControlButton {
    Move,
    Search,
    Toggle,
    SelectAll,
    Clear,
    Expand,
    Download,
    Switch,
    Help,
    Quit,
    Edit,
    Apply,
}

pub(crate) fn select_control_button_at(
    area: ratatui::layout::Rect,
    x: u16,
    y: u16,
    search_active: bool,
    transfer_active: bool,
    selected_count: usize,
) -> Option<SelectControlButton> {
    if y != area.y + 1 || x < area.x + 2 {
        return None;
    }
    let mut cursor = area.x
        + 2
        + select_control_prompt(search_active, transfer_active, selected_count).len() as u16;
    for &(key, label) in select_control_items(search_active, transfer_active, selected_count) {
        let label_width = if label.trim().is_empty() {
            0
        } else {
            1 + label.trim().len() as u16
        };
        let width = 3 + key.len() as u16 + label_width;
        if x >= cursor && x < cursor.saturating_add(width) {
            return control_button_from_item(key, label);
        }
        cursor = cursor.saturating_add(width + 1);
    }
    None
}

fn control_button_from_item(key: &str, _label: &str) -> Option<SelectControlButton> {
    match key {
        "Move" => Some(SelectControlButton::Move),
        "Search" => Some(SelectControlButton::Search),
        "Toggle" => Some(SelectControlButton::Toggle),
        "All" => Some(SelectControlButton::SelectAll),
        "Clear" => Some(SelectControlButton::Clear),
        "Expand" => Some(SelectControlButton::Expand),
        "Download" => Some(SelectControlButton::Download),
        "Transfer" => Some(SelectControlButton::Switch),
        "Help" => Some(SelectControlButton::Help),
        "Quit" => Some(SelectControlButton::Quit),
        "Edit" => Some(SelectControlButton::Edit),
        "Apply" => Some(SelectControlButton::Apply),
        _ => None,
    }
}

pub(crate) fn filtered_album_indices(albums: &[models::AlbumBrief], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    albums
        .iter()
        .enumerate()
        .filter(|(_, album)| query.is_empty() || album.name.to_lowercase().contains(&query))
        .map(|(idx, _)| idx)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectRow {
    Album { album_idx: usize },
    Track { album_idx: usize, track_idx: usize },
}

pub(crate) fn visible_select_rows(
    albums: &[models::AlbumBrief],
    query: &str,
    expanded_album_idx: Option<usize>,
    expanded_album: Option<&models::AlbumDetail>,
) -> Vec<SelectRow> {
    let mut rows = Vec::new();
    for album_idx in filtered_album_indices(albums, query) {
        rows.push(SelectRow::Album { album_idx });
        if Some(album_idx) == expanded_album_idx {
            if let Some(album) = expanded_album {
                rows.extend(album.songs.iter().enumerate().map(|(track_idx, _)| {
                    SelectRow::Track {
                        album_idx,
                        track_idx,
                    }
                }));
            }
        }
    }
    rows
}

pub(crate) fn selected_visible_position(visible_indices: &[usize], selected: usize) -> usize {
    visible_indices
        .iter()
        .position(|&idx| idx == selected)
        .unwrap_or(0)
}

pub(crate) fn ensure_visible_selection(selected: &mut usize, visible_indices: &[usize]) {
    if visible_indices.is_empty() {
        return;
    }
    if !visible_indices.contains(selected) {
        *selected = visible_indices[0];
    }
}

#[cfg(test)]
pub(crate) fn move_selection(selected: &mut usize, visible_indices: &[usize], delta: isize) {
    if visible_indices.is_empty() {
        return;
    }
    let current = selected_visible_position(visible_indices, *selected);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        (current + delta as usize).min(visible_indices.len() - 1)
    };
    *selected = visible_indices[next];
}

pub(crate) struct SelectScreen<'a> {
    pub(crate) albums: &'a [models::AlbumBrief],
    pub(crate) selected: usize,
    pub(crate) selected_track: Option<usize>,
    pub(crate) selected_albums: &'a [bool],
    pub(crate) album_track_selections: &'a [Option<Vec<bool>>],
    pub(crate) album_details: &'a [Option<models::AlbumDetail>],
    pub(crate) expanded_album_idx: Option<usize>,
    pub(crate) expanded_album: Option<&'a models::AlbumDetail>,
    pub(crate) is_downloading: bool,
    pub(crate) search_query: &'a str,
    pub(crate) search_active: bool,
    pub(crate) help_overlay: HelpOverlay,
}

pub(crate) fn draw_select_screen(f: &mut ratatui::Frame, screen: SelectScreen<'_>) {
    let SelectScreen {
        albums,
        selected,
        selected_track,
        selected_albums,
        album_track_selections,
        album_details,
        expanded_album_idx,
        expanded_album,
        is_downloading,
        search_query,
        search_active,
        help_overlay,
    } = screen;

    let chunks = app_chunks(f.area());

    draw_app_header(
        f,
        chunks[0],
        AppScreen::Select,
        "WELCOME TO A WORLD FAMILIARLY UNKNOWN".to_string(),
        COLOR_PRIMARY,
    );

    let body = select_body_chunks(chunks[1]);

    let visible_indices = filtered_album_indices(albums, search_query);
    let visible_rows =
        visible_select_rows(albums, search_query, expanded_album_idx, expanded_album);
    let selected_row = selected_track
        .map(|track_idx| SelectRow::Track {
            album_idx: selected,
            track_idx,
        })
        .unwrap_or(SelectRow::Album {
            album_idx: selected,
        });
    let visible_position = visible_rows
        .iter()
        .position(|row| *row == selected_row)
        .unwrap_or_else(|| selected_visible_position(&visible_indices, selected));

    let items: Vec<ListItem> = visible_rows
        .iter()
        .map(|row| match *row {
            SelectRow::Album { album_idx } => {
                let a = &albums[album_idx];
                let disclosure = if Some(album_idx) == expanded_album_idx {
                    "v"
                } else {
                    ">"
                };
                let checkbox = album_checkbox(album_idx, selected_albums, album_track_selections);
                let style = if selected_track.is_none() && album_idx == selected {
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .bg(Color::Rgb(16, 20, 22))
                        .add_modifier(Modifier::BOLD)
                } else if selected_albums[album_idx] {
                    Style::default().fg(COLOR_SUCCESS)
                } else {
                    Style::default().fg(COLOR_MUTED)
                };
                ListItem::new(Line::from(vec![Span::styled(
                    format!("{} {} {}", disclosure, checkbox, a.name),
                    style,
                )]))
            }
            SelectRow::Track {
                album_idx,
                track_idx,
            } => {
                let song = expanded_album
                    .and_then(|album| album.songs.get(track_idx))
                    .map(|song| song.name.as_str())
                    .unwrap_or("");
                let selected_song = selected_albums[album_idx]
                    || album_track_selections
                        .get(album_idx)
                        .and_then(Option::as_ref)
                        .and_then(|selection| selection.get(track_idx))
                        .copied()
                        .unwrap_or(false);
                let checkbox = if selected_song { "[x]" } else { "[ ]" };
                let style = if selected_track == Some(track_idx) && album_idx == selected {
                    Style::default()
                        .fg(COLOR_SECONDARY)
                        .bg(Color::Rgb(16, 20, 22))
                        .add_modifier(Modifier::BOLD)
                } else if selected_song {
                    Style::default().fg(COLOR_SUCCESS)
                } else {
                    Style::default().fg(COLOR_MUTED)
                };
                ListItem::new(Line::from(vec![Span::styled(
                    format!("    {} {:02}  {}", checkbox, track_idx + 1, song),
                    style,
                )]))
            }
        })
        .collect();

    let list_title = if search_query.is_empty() {
        "BROWSE ALBUMS".to_string()
    } else {
        format!("BROWSE ALBUMS / FILTER {} MATCHES", visible_indices.len())
    };
    let list_block = create_block(&list_title, COLOR_MUTED);
    let list = List::new(items)
        .block(list_block)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let mut list_state = ListState::default();
    if !visible_indices.is_empty() {
        list_state.select(Some(visible_position));
    }
    f.render_stateful_widget(list, body[0], &mut list_state);

    let queue_items = queue_items(
        albums,
        selected_albums,
        album_track_selections,
        album_details,
    );

    if queue_items.is_empty() {
        let empty_queue = Paragraph::new(vec![
            Line::from(vec![
                Span::styled("CLICK [ ]", Style::default().fg(COLOR_PRIMARY)),
                Span::raw(" TO SELECT AN ALBUM"),
            ]),
            Line::from(vec![
                Span::styled("DOUBLE CLICK", Style::default().fg(COLOR_PRIMARY)),
                Span::raw(" AN ALBUM TO PICK TRACKS"),
            ]),
        ])
        .style(Style::default().fg(COLOR_MUTED))
        .block(create_block("DOWNLOAD QUEUE", COLOR_MUTED));
        f.render_widget(empty_queue, body[1]);
    } else {
        let queue = List::new(queue_items).block(create_block("DOWNLOAD QUEUE", COLOR_PRIMARY));
        f.render_widget(queue, body[1]);
    }

    let count = selected_albums
        .iter()
        .enumerate()
        .filter(|(idx, selected_album)| {
            **selected_album
                || album_track_selections
                    .get(*idx)
                    .and_then(Option::as_ref)
                    .is_some_and(|selection| selection.iter().any(|&selected| selected))
        })
        .count();
    let status_text = select_status_text(
        count,
        visible_indices.len(),
        search_query,
        search_active,
        is_downloading,
    );
    draw_status_bar(f, chunks[2], status_text);

    draw_controls_bar(
        f,
        chunks[3],
        select_controls_line(search_active, is_downloading, count),
    );

    if help_overlay == HelpOverlay::Visible {
        draw_help_overlay(f, f.area());
    }
}

fn album_checkbox(
    album_idx: usize,
    selected_albums: &[bool],
    album_track_selections: &[Option<Vec<bool>>],
) -> &'static str {
    if selected_albums[album_idx] {
        return "[x]";
    }
    let Some(selection) = album_track_selections
        .get(album_idx)
        .and_then(Option::as_ref)
    else {
        return "[ ]";
    };
    if selection.iter().any(|&selected| selected) {
        if selection.iter().all(|&selected| selected) {
            "[x]"
        } else {
            "[-]"
        }
    } else {
        "[ ]"
    }
}

fn queue_items<'a>(
    albums: &'a [models::AlbumBrief],
    selected_albums: &[bool],
    album_track_selections: &[Option<Vec<bool>>],
    album_details: &'a [Option<models::AlbumDetail>],
) -> Vec<ListItem<'a>> {
    let mut items = Vec::new();
    for (album_idx, album) in albums.iter().enumerate() {
        if selected_albums[album_idx] {
            items.push(ListItem::new(vec![
                Line::from(vec![
                    Span::styled("[x] ", Style::default().fg(COLOR_SUCCESS)),
                    Span::styled(album.name.clone(), Style::default().fg(COLOR_SECONDARY)),
                ]),
                Line::from(Span::styled(
                    "    full album",
                    Style::default().fg(COLOR_MUTED),
                )),
            ]));
            continue;
        }

        let Some(selection) = album_track_selections
            .get(album_idx)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        let selected_count = selection.iter().filter(|&&selected| selected).count();
        if selected_count == 0 {
            continue;
        }
        let total_count = selection.len();
        let mut lines = vec![
            Line::from(vec![
                Span::styled("[-] ", Style::default().fg(COLOR_PRIMARY)),
                Span::styled(album.name.clone(), Style::default().fg(COLOR_SECONDARY)),
            ]),
            Line::from(Span::styled(
                format!("    {selected_count}/{total_count} tracks"),
                Style::default().fg(COLOR_MUTED),
            )),
        ];
        if let Some(detail) = album_details.get(album_idx).and_then(Option::as_ref) {
            lines.extend(
                detail
                    .songs
                    .iter()
                    .zip(selection.iter())
                    .enumerate()
                    .filter(|(_, (_, selected))| **selected)
                    .map(|(track_idx, (song, _))| {
                        Line::from(Span::styled(
                            format!("    {:02} {}", track_idx + 1, song.name),
                            Style::default().fg(COLOR_MUTED),
                        ))
                    }),
            );
        }
        items.push(ListItem::new(lines));
    }
    items
}

fn select_controls_line(
    search_active: bool,
    transfer_active: bool,
    selected_count: usize,
) -> Line<'static> {
    let mut spans = vec![Span::raw(select_control_prompt(
        search_active,
        transfer_active,
        selected_count,
    ))];
    for &(key, _) in select_control_items(search_active, transfer_active, selected_count) {
        spans.push(Span::raw("["));
        spans.push(Span::styled(key, Style::default().fg(COLOR_PRIMARY)));
        spans.push(Span::raw("] "));
    }
    Line::from(spans)
}

fn select_control_prompt(
    search_active: bool,
    transfer_active: bool,
    selected_count: usize,
) -> &'static str {
    if search_active {
        SELECT_SEARCH_PROMPT
    } else if transfer_active {
        SELECT_TRANSFER_PROMPT
    } else if selected_count == 0 {
        SELECT_IDLE_PROMPT
    } else {
        SELECT_READY_PROMPT
    }
}

pub(crate) fn select_status_text(
    selected_count: usize,
    visible_count: usize,
    search_query: &str,
    search_active: bool,
    transfer_active: bool,
) -> String {
    if search_active {
        if visible_count == 0 {
            format!("FILTER \"{}\" / NO MATCHES", search_query)
        } else {
            format!("FILTER \"{}\" / {} MATCHES", search_query, visible_count)
        }
    } else if transfer_active {
        format!(
            "TRANSFER ACTIVE / {} ALBUM{} QUEUED",
            selected_count,
            if selected_count == 1 { "" } else { "S" }
        )
    } else if selected_count == 0 {
        "NO ALBUM SELECTED".to_string()
    } else {
        format!(
            "{} ALBUM{} READY",
            selected_count,
            if selected_count == 1 { "" } else { "S" }
        )
    }
}

#[cfg(test)]
pub(crate) fn select_controls_text(
    search_active: bool,
    transfer_active: bool,
    selected_count: usize,
) -> String {
    crate::tui::chrome::controls_text(select_control_items(
        search_active,
        transfer_active,
        selected_count,
    ))
}

fn select_control_items(
    search_active: bool,
    transfer_active: bool,
    selected_count: usize,
) -> &'static [(&'static str, &'static str)] {
    if search_active {
        SELECT_SEARCH_CONTROLS
    } else if transfer_active {
        SELECT_TRANSFER_CONTROLS
    } else if selected_count == 0 {
        SELECT_EMPTY_CONTROLS
    } else {
        SELECT_READY_CONTROLS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_status_stays_status_only() {
        assert_eq!(
            select_status_text(0, 5, "", false, false),
            "NO ALBUM SELECTED"
        );
        assert_eq!(select_status_text(1, 5, "", false, false), "1 ALBUM READY");
        assert_eq!(select_status_text(3, 5, "", false, false), "3 ALBUMS READY");
        assert_eq!(
            select_status_text(3, 5, "ark", true, false),
            "FILTER \"ark\" / 5 MATCHES"
        );
        assert_eq!(
            select_status_text(3, 0, "ark", true, false),
            "FILTER \"ark\" / NO MATCHES"
        );
        assert_eq!(
            select_status_text(2, 5, "", false, true),
            "TRANSFER ACTIVE / 2 ALBUMS QUEUED"
        );
    }

    #[test]
    fn select_controls_follow_interaction_mode() {
        assert_eq!(select_controls_text(true, false, 0), "[Apply] [Help] ");
        assert_eq!(select_controls_text(false, true, 2), "[Search] [Help] ");
        assert!(!select_controls_text(false, false, 0).contains("[Quit]"));
        assert!(!select_controls_text(false, false, 0).contains("[Download]"));
        assert!(select_controls_text(false, false, 1).contains("[Download]"));
        assert!(!select_controls_text(false, true, 2).contains("[Expand]"));
        assert!(!select_controls_text(false, false, 0).contains("[Expand]"));
        assert!(!select_controls_text(false, false, 0).contains("[Clear]"));
        assert!(select_controls_text(false, false, 1).contains("[Clear]"));
    }

    #[test]
    fn select_control_hit_testing_uses_rendered_padding() {
        let area = ratatui::layout::Rect::new(0, 10, 100, 3);
        let search_x = area.x + 2 + SELECT_IDLE_PROMPT.len() as u16 + 1;

        assert_eq!(
            select_control_button_at(area, search_x, area.y + 1, false, false, 0),
            Some(SelectControlButton::Search)
        );
        assert_eq!(
            select_control_button_at(area, search_x, area.y + 1, true, false, 0),
            None
        );
        assert_eq!(
            select_control_button_at(area, area.x + 1, area.y + 1, false, false, 0),
            None
        );
        assert_eq!(
            select_control_button_at(area, search_x, area.y, false, false, 0),
            None
        );
    }
}
