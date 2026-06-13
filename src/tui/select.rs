use crate::models;
use crate::tui::chrome::{create_block, draw_app_header, draw_controls_bar, draw_status_bar};
use crate::tui::layout::{app_chunks, select_body_chunks};
use crate::tui::overlay::draw_help_overlay;
use crate::tui::state::{AppScreen, HelpOverlay};
use crate::tui::theme::{COLOR_INFO, COLOR_MUTED, COLOR_PRIMARY, COLOR_SECONDARY, COLOR_SUCCESS};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph},
};

pub(crate) fn filtered_album_indices(albums: &[models::AlbumBrief], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    albums
        .iter()
        .enumerate()
        .filter(|(_, album)| query.is_empty() || album.name.to_lowercase().contains(&query))
        .map(|(idx, _)| idx)
        .collect()
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

pub(crate) fn draw_select_screen(
    f: &mut ratatui::Frame,
    albums: &[models::AlbumBrief],
    selected: usize,
    selected_albums: &[bool],
    is_downloading: bool,
    search_query: &str,
    search_active: bool,
    help_overlay: HelpOverlay,
) {
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
    let visible_position = selected_visible_position(&visible_indices, selected);

    let items: Vec<ListItem> = visible_indices
        .iter()
        .map(|&i| {
            let a = &albums[i];
            let focus = if i == selected { ">" } else { " " };
            let checkbox = if selected_albums[i] { "[x]" } else { "[ ]" };
            let style = if i == selected {
                Style::default()
                    .fg(COLOR_SECONDARY)
                    .bg(Color::Rgb(16, 20, 22))
                    .add_modifier(Modifier::BOLD)
            } else if selected_albums[i] {
                Style::default().fg(COLOR_SUCCESS)
            } else {
                Style::default().fg(COLOR_MUTED)
            };

            ListItem::new(Line::from(vec![Span::styled(
                format!("{} {} {}", focus, checkbox, a.name),
                style,
            )]))
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

    let queue_items: Vec<ListItem> = albums
        .iter()
        .enumerate()
        .filter(|(idx, _)| selected_albums[*idx])
        .map(|(_, album)| {
            ListItem::new(Line::from(vec![
                Span::styled("+ ", Style::default().fg(COLOR_PRIMARY)),
                Span::styled(album.name.clone(), Style::default().fg(COLOR_SECONDARY)),
            ]))
        })
        .collect();

    if queue_items.is_empty() {
        let empty_queue = Paragraph::new(Line::from(vec![
            Span::styled("SPACE", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(" TO ADD AN ALBUM\n"),
            Span::styled("ENTER", Style::default().fg(COLOR_PRIMARY)),
            Span::raw(" TO START WHEN READY"),
        ]))
        .style(Style::default().fg(COLOR_MUTED))
        .block(create_block("DOWNLOAD QUEUE", COLOR_MUTED));
        f.render_widget(empty_queue, body[1]);
    } else {
        let queue = List::new(queue_items).block(create_block("DOWNLOAD QUEUE", COLOR_PRIMARY));
        f.render_widget(queue, body[1]);
    }

    let count = selected_albums.iter().filter(|&&s| s).count();
    let status_text = if search_active {
        if visible_indices.is_empty() {
            format!("FILTER: {}_ / NO MATCHES / ESC CLEARS", search_query)
        } else {
            format!(
                "FILTER: {}_ / {} MATCHES",
                search_query,
                visible_indices.len()
            )
        }
    } else if is_downloading {
        format!("TRANSFER ACTIVE / {} IN QUEUE / TAB TO PROGRESS", count)
    } else if count == 0 {
        "NO ALBUM SELECTED".to_string()
    } else {
        format!("{} ALBUM{} READY", count, if count > 1 { "S" } else { "" })
    };
    draw_status_bar(f, chunks[2], status_text);

    draw_controls_bar(
        f,
        chunks[3],
        Line::from(vec![
            Span::styled("↑↓", Style::default().fg(COLOR_INFO)),
            Span::raw(" MOVE  "),
            Span::styled("Space", Style::default().fg(COLOR_INFO)),
            Span::raw(" SELECT  "),
            Span::styled("A", Style::default().fg(COLOR_INFO)),
            Span::raw(" ALL  "),
            Span::styled("C", Style::default().fg(COLOR_INFO)),
            Span::raw(" CLEAR  "),
            Span::styled("/", Style::default().fg(COLOR_INFO)),
            Span::raw(" SEARCH  "),
            Span::styled("Esc", Style::default().fg(COLOR_INFO)),
            Span::raw(" CLEAR FILTER  "),
            Span::styled("Enter", Style::default().fg(COLOR_INFO)),
            Span::raw(" DOWNLOAD  "),
            Span::styled("Tab", Style::default().fg(COLOR_INFO)),
            Span::raw(" PROGRESS  "),
            Span::styled("?", Style::default().fg(COLOR_INFO)),
            Span::raw(" HELP  "),
            Span::styled("Q", Style::default().fg(COLOR_INFO)),
            Span::raw(" QUIT"),
        ]),
    );

    if help_overlay == HelpOverlay::Visible {
        draw_help_overlay(f, chunks[1]);
    }
}
