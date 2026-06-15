use crate::tui::state::AppScreen;
use crate::tui::theme::COLOR_PRIMARY;
use crate::tui::theme::{COLOR_MUTED, COLOR_SECONDARY};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph},
};

pub(crate) fn create_block(title: &str, border_color: Color) -> Block<'_> {
    Block::default()
        .title(format!(" {} ", title.to_ascii_uppercase()))
        .title_style(
            Style::default()
                .fg(COLOR_SECONDARY)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1))
}

pub(crate) fn draw_app_header(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    screen: AppScreen,
    title_text: String,
    title_color: Color,
) {
    let title = Paragraph::new(Line::from(vec![
        tab_span("ALBUMS [1]", screen == AppScreen::Select),
        Span::raw("    "),
        tab_span("TRANSFER [2]", screen == AppScreen::Downloading),
        Span::raw("    "),
        Span::styled(
            title_text,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(create_block("MONSTER SIREN RECORDS", title_color));
    f.render_widget(title, area);
}

pub(crate) fn draw_status_bar(f: &mut ratatui::Frame, area: ratatui::layout::Rect, text: String) {
    let status = Paragraph::new(text)
        .style(Style::default().fg(COLOR_MUTED))
        .block(create_block("STATUS", COLOR_MUTED));
    f.render_widget(status, area);
}

pub(crate) fn draw_controls_bar(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    line: Line<'static>,
) {
    let controls = Paragraph::new(line).block(create_block("CONTROLS", COLOR_MUTED));
    f.render_widget(controls, area);
}

#[cfg(test)]
pub(crate) fn controls_text(items: &[(&str, &str)]) -> String {
    items.iter().fold(String::new(), |mut text, (key, label)| {
        text.push('[');
        text.push_str(key);
        let label = label.trim();
        if !label.is_empty() {
            text.push(' ');
            text.push_str(label);
        }
        text.push_str("] ");
        text
    })
}

fn tab_span(label: &'static str, active: bool) -> Span<'static> {
    if active {
        Span::styled(
            label,
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(label, Style::default().fg(COLOR_MUTED))
    }
}
