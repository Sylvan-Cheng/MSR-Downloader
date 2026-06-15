use crate::tui::chrome::create_block;
use crate::tui::layout::{centered_rect, contains_point};
use crate::tui::theme::{COLOR_INFO, COLOR_PANEL, COLOR_PRIMARY};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Wrap},
};

pub(crate) fn draw_help_overlay(f: &mut ratatui::Frame, area: Rect) {
    let popup = centered_rect(70, 70, area);
    let lines = vec![
        Line::from(Span::styled(
            "KEYBOARD HELP",
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::raw("Albums: Up/Down move, PageUp/PageDown jump, Home/End first/last"),
        Line::raw("Select: Space toggles one album, A toggles filtered albums, C clears queue"),
        Line::raw("Search: / starts typing, Enter applies, Esc clears filter"),
        Line::raw("Transfer: Tab switches between Albums and Transfer pages"),
        Line::raw("Abort: Q asks before aborting active work, Y confirms, N/Esc cancels"),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Esc", Style::default().fg(COLOR_INFO)),
            Span::raw(" CLOSE HELP"),
        ]),
    ];
    let help = Paragraph::new(lines)
        .style(Style::default().bg(COLOR_PANEL))
        .block(create_block("HELP", COLOR_PRIMARY))
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, popup);
    f.render_widget(help, popup);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("["),
            Span::styled("X", Style::default().fg(COLOR_PRIMARY)),
            Span::raw("]"),
        ]))
        .style(Style::default().bg(COLOR_PANEL)),
        help_close_button(area),
    );
}

pub(crate) fn help_close_button(area: Rect) -> Rect {
    let popup = centered_rect(70, 70, area);
    Rect::new(popup.x + popup.width.saturating_sub(5), popup.y, 3, 1)
}

pub(crate) fn is_help_close_click(area: Rect, x: u16, y: u16) -> bool {
    contains_point(help_close_button(area), x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_close_click_matches_rendered_button() {
        let area = Rect::new(0, 0, 120, 40);
        let button = help_close_button(area);

        assert_eq!(button.width, 3);
        assert!(is_help_close_click(area, button.x, button.y));
        assert!(is_help_close_click(
            area,
            button.x + button.width - 1,
            button.y
        ));
        assert!(!is_help_close_click(area, button.x - 1, button.y));
        assert!(!is_help_close_click(area, button.x, button.y + 1));
    }
}
