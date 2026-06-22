//! Logs view: shows the tail of the TUI's log file and its path.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::commands::tui::app::App;

pub(crate) fn on_key(app: &mut App, key: KeyEvent) {
    match key.code {
        // Scroll up into history (increases offset from the bottom).
        KeyCode::Up | KeyCode::Char('k') => {
            app.logs.scroll_from_bottom = app.logs.scroll_from_bottom.saturating_add(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.logs.scroll_from_bottom = app.logs.scroll_from_bottom.saturating_sub(1);
        }
        KeyCode::PageUp => {
            app.logs.scroll_from_bottom = app.logs.scroll_from_bottom.saturating_add(20);
        }
        KeyCode::PageDown => {
            app.logs.scroll_from_bottom = app.logs.scroll_from_bottom.saturating_sub(20);
        }
        // `G` jumps to the bottom (follow the tail); `g` jumps to the top.
        KeyCode::Char('G') => app.logs.scroll_from_bottom = 0,
        KeyCode::Char('g') => {
            app.logs.scroll_from_bottom = app.logs.lines.len();
        }
        // Reload the file now.
        KeyCode::Char('R') => app.load_logs(),
        _ => {}
    }
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    render_path(app, frame, chunks[0]);
    render_body(app, frame, chunks[1]);
}

fn render_path(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let line = match &app.logs.path {
        Some(path) => Line::from(vec![
            Span::styled(" Log file: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                path.display().to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        None => Line::from(Span::styled(
            " Logs are written by the remote node when using --rpc-url.",
            Style::default().fg(Color::DarkGray),
        )),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_body(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let following = app.logs.scroll_from_bottom == 0;
    let title = format!(
        " Logs  ({}  ·  j/k scroll · g/G top/bottom · R reload) ",
        if following { "following" } else { "scrolled" },
    );
    let block = Block::default().borders(Borders::ALL).title(title);

    if let Some(err) = &app.logs.read_error {
        let p = Paragraph::new(err.clone())
            .block(block)
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: true });
        frame.render_widget(p, area);
        return;
    }

    // The visible window is the last `height` lines, offset upward by `scroll_from_bottom`.
    let height = area.height.saturating_sub(2) as usize; // account for borders
    let total = app.logs.lines.len();

    // Clamp the scroll so we never go past the top.
    let max_scroll = total.saturating_sub(height);
    let scroll = app.logs.scroll_from_bottom.min(max_scroll);

    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(height);

    let lines: Vec<Line<'_>> = app.logs.lines[start..end]
        .iter()
        .map(|l| Line::from(Span::raw(l.clone())))
        .collect();

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}
