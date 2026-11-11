use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};

pub mod statusbar;
pub mod tree_pane;

const DIFF_PLACEHOLDER: &str = "Select a file and press Enter";

/// Vertical: [tree | diff, statusbar]; the main area splits 45/55.
pub fn draw(frame: &mut Frame, app: &App) {
    let chunks =
        Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).split(frame.area());
    let panes = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[0]);
    tree_pane::render(frame, panes[0], app);
    let border_style = if app.focus() == Focus::Diff {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };
    let placeholder = Paragraph::new(DIFF_PLACEHOLDER)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Diff")
                .border_style(border_style),
        );
    frame.render_widget(placeholder, panes[1]);
    statusbar::render(frame, chunks[1], app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use ratatui::{backend::TestBackend, Terminal};

    use crate::app::App;
    use crate::config::Config;
    use crate::git::status::{FileStatus, GitState, StatusKind};

    #[test]
    fn draw_smoke_test_backend() {
        let mut files = BTreeMap::new();
        files.insert(
            PathBuf::from("src/main.rs"),
            FileStatus {
                path: PathBuf::from("src/main.rs"),
                index: '.',
                worktree: 'M',
                kind: StatusKind::Modified,
                orig_path: None,
                staged: false,
            },
        );
        let git = GitState {
            branch: Some("main".to_string()),
            files,
            additions: 12,
            deletions: 4,
        };
        let app = App::new(
            PathBuf::from("/repo"),
            Config::default(),
            git,
            vec![PathBuf::from("README.md"), PathBuf::from("src/main.rs")],
        );

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();

        let buffer = terminal.backend_mut().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("agtx"), "statusbar must show agtx");
        assert!(text.contains('M'), "modified file must show its symbol");
    }
}
