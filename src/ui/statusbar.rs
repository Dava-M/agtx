use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::App;

const HINTS: &str = "j/k·Enter diff·S-Enter/p file·- fold·f changed·s staged·r refresh·e edit·q quit";

/// Bottom two lines: summary with change counts, then key hints.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let lines = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    render_summary(frame, lines[0], app);
    frame.render_widget(
        Paragraph::new(HINTS).style(Style::default().add_modifier(Modifier::DIM)),
        lines[1],
    );
}

fn render_summary(frame: &mut Frame, area: Rect, app: &App) {
    let git = app.git();
    let branch = git
        .branch
        .clone()
        .unwrap_or_else(|| "(no branch)".to_string());
    let mut left = format!("agtx {} ({branch})", app.root().display());
    let status = app.status_message();
    if !status.is_empty() {
        left.push_str(&format!(" | {status}"));
    }
    let right = format!("{} files · +{} −{}", git.files.len(), git.additions, git.deletions);
    let chunks = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(right.chars().count() as u16),
    ])
    .split(area);
    frame.render_widget(Paragraph::new(left), chunks[0]);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        chunks[1],
    );
}
