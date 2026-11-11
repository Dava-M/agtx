use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::git::diff::{DiffLine, DiffLineKind};

const NO_SELECTION: &str = "Select a file and press Enter";
const NO_CHANGES: &str = "No uncommitted changes";
const LINE_NO_WIDTH: usize = 5;

/// Right pane: the unified diff of the selected file.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    app.set_diff_viewport_height(area.height.saturating_sub(2));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title(app))
        .border_style(border_style(app));

    let paragraph = match app.diff() {
        None => Paragraph::new(NO_SELECTION)
            .alignment(Alignment::Center)
            .block(block),
        Some(view) if view.lines.is_empty() => Paragraph::new(NO_CHANGES)
            .alignment(Alignment::Center)
            .block(block),
        Some(view) => {
            let mut lines: Vec<Line> = Vec::new();
            if view.binary {
                lines.push(Line::styled("binary file", style_for(DiffLineKind::Meta)));
            }
            if view.truncated {
                lines.push(Line::styled("diff truncated", style_for(DiffLineKind::Meta)));
            }
            lines.extend(
                view.lines
                    .iter()
                    .map(|line| Line::styled(render_line(line), style_for(line.kind))),
            );
            Paragraph::new(lines)
                .block(block)
                .scroll((app.diff_scroll().min(u16::MAX as usize) as u16, 0))
        }
    };
    frame.render_widget(paragraph, area);
}

fn title(app: &App) -> String {
    match app.diff() {
        Some(view) => format!(
            "Diff: {} [{}]",
            view.path.display(),
            if view.staged { "staged" } else { "worktree" }
        ),
        None => "Diff".to_string(),
    }
}

fn border_style(app: &App) -> Style {
    if app.focus() == Focus::Diff {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    }
}

fn style_for(kind: DiffLineKind) -> Style {
    match kind {
        DiffLineKind::Header | DiffLineKind::Hunk => Style::default().fg(Color::Cyan),
        DiffLineKind::Add => Style::default().fg(Color::Green),
        DiffLineKind::Del => Style::default().fg(Color::Red),
        DiffLineKind::Context => Style::default(),
        DiffLineKind::Meta => Style::default().add_modifier(Modifier::DIM),
    }
}

/// Two right-aligned line-number columns, then the raw diff text.
fn render_line(line: &DiffLine) -> String {
    let old = line.old_no.map(|n| n.to_string()).unwrap_or_default();
    let new = line.new_no.map(|n| n.to_string()).unwrap_or_default();
    format!(
        "{old:>width$}{new:>width$} {}",
        line.text,
        width = LINE_NO_WIDTH
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use ratatui::{backend::TestBackend, Terminal};

    use crate::config::Config;
    use crate::git::diff::parse;
    use crate::git::status::GitState;

    fn app_with(text: &str, truncated: bool) -> App {
        let git = GitState {
            branch: Some("main".to_string()),
            files: BTreeMap::new(),
            additions: 1,
            deletions: 1,
        };
        let mut app = App::new(PathBuf::from("/repo"), Config::default(), git, Vec::new());
        app.preload_diff(parse(text, PathBuf::from("src/main.rs"), false, truncated));
        app
    }

    fn render_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        terminal
            .backend_mut()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_path_and_add_del_lines() {
        let text = [
            "diff --git a/src/main.rs b/src/main.rs",
            "index 111..222 100644",
            "--- a/src/main.rs",
            "+++ b/src/main.rs",
            "@@ -1,2 +1,2 @@",
            " context line",
            "-old line",
            "+new line",
        ]
        .join("\n");
        let app = app_with(&text, false);
        let rendered = render_text(&app);

        assert!(rendered.contains("src/main.rs"), "path in title: {rendered}");
        assert!(rendered.contains("[worktree]"), "worktree marker: {rendered}");
        assert!(rendered.contains("+new line"), "added line: {rendered}");
        assert!(rendered.contains("-old line"), "deleted line: {rendered}");
        assert!(rendered.contains("@@ -1,2 +1,2 @@"), "hunk header: {rendered}");
    }

    #[test]
    fn renders_binary_truncated_and_empty_notices() {
        let app = app_with(
            "diff --git a/x b/x\nBinary files a/x and b/x differ\n",
            true,
        );
        let rendered = render_text(&app);
        assert!(rendered.contains("binary file"), "{rendered}");
        assert!(rendered.contains("diff truncated"), "{rendered}");

        let app = app_with("", false);
        let rendered = render_text(&app);
        assert!(rendered.contains(NO_CHANGES), "{rendered}");
    }
}
