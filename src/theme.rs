use ratatui::style::{Color, Modifier, Style};

use crate::git::status::{FileStatus, StatusKind};

/// One-letter symbol shown next to a changed file.
pub fn file_symbol(status: &FileStatus) -> &'static str {
    match status.kind {
        StatusKind::Modified => "M",
        StatusKind::Added => "A",
        StatusKind::Deleted => "D",
        StatusKind::Renamed => "R",
        StatusKind::Copied => "C",
        StatusKind::Unmerged => "U",
        StatusKind::Untracked => "?",
        StatusKind::Unknown => " ",
    }
}

/// Color of a changed file entry.
pub fn file_style(status: &FileStatus) -> Style {
    match status.kind {
        StatusKind::Modified => Style::default().fg(Color::Yellow),
        StatusKind::Added | StatusKind::Untracked => Style::default().fg(Color::Green),
        StatusKind::Deleted => Style::default().fg(Color::Red),
        StatusKind::Renamed | StatusKind::Copied => Style::default().fg(Color::Cyan),
        StatusKind::Unmerged => Style::default().fg(Color::Magenta),
        StatusKind::Unknown => Style::default(),
    }
}

/// Directories are bold.
pub fn dir_style() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

/// The selected tree row is drawn reversed.
pub fn selected_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// Files touched in the last 10 seconds.
pub fn recent_style() -> Style {
    Style::default()
        .fg(Color::LightGreen)
        .add_modifier(Modifier::BOLD)
}
