use msr_downloader::progress::SongStatus;
use ratatui::style::{Color, Modifier, Style};

pub(crate) const COLOR_PRIMARY: Color = Color::Rgb(0, 216, 198);
pub(crate) const COLOR_SECONDARY: Color = Color::Rgb(214, 218, 216);
pub(crate) const COLOR_SUCCESS: Color = Color::Rgb(0, 216, 198);
pub(crate) const COLOR_WARNING: Color = Color::Rgb(214, 218, 216);
pub(crate) const COLOR_ERROR: Color = Color::Rgb(238, 89, 82);
pub(crate) const COLOR_INFO: Color = Color::Rgb(0, 216, 198);
pub(crate) const COLOR_MUTED: Color = Color::Rgb(92, 98, 100);
pub(crate) const COLOR_PANEL: Color = Color::Rgb(16, 20, 22);

pub(crate) fn tui_status_style(status: SongStatus) -> Style {
    match status {
        SongStatus::Failed => Style::default()
            .fg(COLOR_ERROR)
            .add_modifier(Modifier::BOLD),
        SongStatus::Skipped => Style::default()
            .fg(COLOR_WARNING)
            .add_modifier(Modifier::BOLD),
        SongStatus::Done | SongStatus::Resuming => Style::default()
            .fg(COLOR_SUCCESS)
            .add_modifier(Modifier::BOLD),
        SongStatus::Checking | SongStatus::Tagging => Style::default()
            .fg(COLOR_SECONDARY)
            .add_modifier(Modifier::BOLD),
        SongStatus::Queued => Style::default().fg(COLOR_MUTED),
        SongStatus::Getting => Style::default().fg(COLOR_INFO).add_modifier(Modifier::BOLD),
    }
}
