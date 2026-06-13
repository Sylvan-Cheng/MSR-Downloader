use crate::tui::chrome::create_block;
use crate::tui::layout::centered_rect;
use crate::tui::theme::{COLOR_INFO, COLOR_PRIMARY};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
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
        Line::raw("Transfer: 1/2 or Tab switches pages"),
        Line::raw("Abort: Q asks before aborting active work, Y confirms, N/Esc cancels"),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Esc", Style::default().fg(COLOR_INFO)),
            Span::raw(" CLOSE HELP"),
        ]),
    ];
    let help = Paragraph::new(lines)
        .block(create_block("HELP", COLOR_PRIMARY))
        .wrap(Wrap { trim: true });
    f.render_widget(help, popup);
}
