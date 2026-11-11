use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::theme;
use crate::tree::Row;

/// Left pane: the repository file tree.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .map(|row| ListItem::new(row_line(app, row)))
        .collect();
    let border_style = if app.focus() == Focus::Tree {
        Style::default().fg(Color::Green)
    } else {
        Style::default()
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Files")
                .border_style(border_style),
        )
        .highlight_style(theme::selected_style());
    let mut state = ListState::default().with_selected(Some(app.selected_row()));
    frame.render_stateful_widget(list, area, &mut state);
}

fn row_line<'a>(app: &'a App, row: &'a Row) -> Line<'a> {
    let mut spans: Vec<Span> = vec![Span::raw("  ".repeat(row.depth))];
    if row.is_dir {
        let symbol = if row.collapsed { "▸" } else { "▾" };
        spans.push(Span::styled(symbol, theme::dir_style()));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(row.name.clone(), theme::dir_style()));
        let counts = &row.counts;
        if counts.changed > 0 || counts.added > 0 || counts.deleted > 0 {
            spans.push(Span::raw(format!(
                " ({}~ {}+ {}-)",
                counts.changed, counts.added, counts.deleted
            )));
        }
    } else {
        let recent = app.is_recent(&row.path);
        if recent {
            spans.push(Span::styled("●", theme::recent_style()));
        }
        match &row.status {
            Some(status) => {
                spans.push(Span::styled(
                    theme::file_symbol(status),
                    theme::file_style(status),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(row.name.clone(), theme::file_style(status)));
            }
            None => {
                spans.push(Span::raw("  "));
                spans.push(Span::raw(row.name.clone()));
            }
        }
    }
    Line::from(spans)
}
