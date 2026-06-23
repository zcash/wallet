//! Top-level rendering: tab bar, status bar, modals, and view dispatch.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap},
};

use super::app::{App, Focus, View};
use super::client::LockState;
use super::views;

/// Renders the entire UI for one frame.
pub(super) fn render(app: &App, frame: &mut Frame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tab bar
            Constraint::Length(1), // sync bar (visible on every screen)
            Constraint::Min(0),    // body
            Constraint::Length(1), // status/footer
        ])
        .split(frame.area());

    render_tabs(app, frame, chunks[0]);
    render_sync_bar(app, frame, chunks[1]);

    // When the wallet is locked, the body is replaced by a mandatory unlock screen so the
    // wallet never appears usable. The Logs view is exempt: it shows no wallet data and is
    // useful for diagnosing why the wallet may be locked or failing to start.
    if app.is_gated() && app.view != View::Logs {
        render_locked(app, frame, chunks[2]);
    } else {
        render_body(app, frame, chunks[2]);
    }
    render_footer(app, frame, chunks[3]);

    if app.show_help {
        render_help(frame);
    }
    if let Some(prompt) = &app.prompt {
        render_prompt(prompt, frame);
    }
}

/// Renders a compact, always-visible wallet-wide sync progress bar.
fn render_sync_bar(app: &App, frame: &mut Frame<'_>, area: Rect) {
    use ratatui::widgets::LineGauge;

    let summary = app.sync_summary();

    // Build a descriptive label shown alongside the bar.
    let mut label = summary.short_label();
    if let (Some(synced), Some(node)) = (summary.synced_height, summary.node_height) {
        label.push_str("  ");
        label.push_str(&crate::fl!(
            "tui-sync-heights",
            synced = synced,
            node = node
        ));
    } else if let Some(node) = summary.node_height {
        label.push_str("  ");
        label.push_str(&crate::fl!("tui-sync-tip", node = node));
    }
    if let Some(remaining) = summary.unscanned_blocks {
        if remaining > 0 {
            label.push_str("  ");
            label.push_str(&crate::fl!("tui-sync-blocks-left", remaining = remaining));
        }
    }

    let ratio = summary.fraction.unwrap_or(0.0);
    let color = if summary.synced {
        Color::Green
    } else {
        Color::Cyan
    };

    let gauge = LineGauge::default()
        .filled_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .unfilled_style(Style::default().fg(Color::DarkGray))
        .label(format!(" {label} "))
        .ratio(ratio);
    frame.render_widget(gauge, area);
}

fn render_tabs(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let titles: Vec<Line<'_>> = View::ALL
        .iter()
        .enumerate()
        .map(|(i, v)| Line::from(format!(" {}:{} ", i + 1, v.title())))
        .collect();

    let selected = View::ALL.iter().position(|&v| v == app.view).unwrap_or(0);

    // The header is highlighted (cyan border) when focus is on the tab row.
    let border_style = match app.focus {
        Focus::Tabs => Style::default().fg(Color::Cyan),
        Focus::View => Style::default().fg(Color::DarkGray),
    };
    let highlight_style = match app.focus {
        Focus::Tabs => Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        Focus::View => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    };

    let title = format!(
        " {} ",
        crate::fl!("tui-header-title", lock = lock_label(app.lock_state))
    );
    let tabs = Tabs::new(titles)
        .select(selected)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title)
                .title_alignment(Alignment::Left),
        )
        .highlight_style(highlight_style);
    frame.render_widget(tabs, area);
}

fn lock_label(state: LockState) -> String {
    match state {
        LockState::Unencrypted => crate::fl!("tui-lock-unencrypted"),
        LockState::Locked => crate::fl!("tui-lock-locked"),
        LockState::Unlocked => crate::fl!("tui-lock-unlocked"),
    }
}

fn render_locked(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", crate::fl!("tui-locked-title")))
        .border_style(Style::default().fg(Color::Yellow));

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", crate::fl!("tui-locked-line1")),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("  {}", crate::fl!("tui-locked-line2"))),
        Line::from(format!("  {}", crate::fl!("tui-locked-line3"))),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", crate::fl!("tui-locked-hint")),
            Style::default().fg(Color::Cyan),
        )),
    ];

    if app.prompt.is_none() {
        if let Some(toast) = &app.toast {
            if toast.is_error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("  {}", toast.text),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_body(app: &App, frame: &mut Frame<'_>, area: Rect) {
    match app.view {
        View::Dashboard => views::dashboard::render(app, frame, area),
        View::Accounts => views::accounts::render(app, frame, area),
        View::Balances => views::balances::render(app, frame, area),
        View::Addresses => views::addresses::render(app, frame, area),
        View::Transactions => views::transactions::render(app, frame, area),
        View::Send => views::send::render(app, frame, area),
        View::Logs => views::logs::render(app, frame, area),
    }
}

fn render_footer(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let hint = if app.is_gated() {
        crate::fl!("tui-footer-gated")
    } else {
        let lock_hint = match app.lock_state {
            LockState::Unencrypted => String::new(),
            LockState::Locked => crate::fl!("tui-footer-unlock"),
            LockState::Unlocked => crate::fl!("tui-footer-lock"),
        };
        let nav = match app.focus {
            Focus::Tabs => crate::fl!("tui-footer-nav-tabs"),
            Focus::View => crate::fl!("tui-footer-nav-view"),
        };
        crate::fl!("tui-footer-hint", nav = nav, lock = lock_hint)
    };

    let mut spans = vec![Span::styled(
        format!(" {hint} "),
        Style::default().fg(Color::DarkGray),
    )];

    if let Some(toast) = &app.toast {
        let style = if toast.is_error {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        spans.push(Span::styled(format!("— {}", toast.text), style));
    }

    let footer = Paragraph::new(Line::from(spans));
    frame.render_widget(footer, area);
}

fn render_help(frame: &mut Frame<'_>) {
    let area = centered_rect(60, 70, frame.area());
    frame.render_widget(Clear, area);

    let lines = vec![
        Line::from(Span::styled(
            crate::fl!("tui-help-nav-header"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("  {}", crate::fl!("tui-help-nav-esc"))),
        Line::from(format!("  {}", crate::fl!("tui-help-nav-switch-tabs"))),
        Line::from(format!("  {}", crate::fl!("tui-help-nav-enter"))),
        Line::from(format!("  {}", crate::fl!("tui-help-nav-switch-view"))),
        Line::from(format!("  {}", crate::fl!("tui-help-nav-jump"))),
        Line::from(format!("  {}", crate::fl!("tui-help-nav-select"))),
        Line::from(""),
        Line::from(Span::styled(
            crate::fl!("tui-help-global-header"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("  {}", crate::fl!("tui-help-global-refresh"))),
        Line::from(format!("  {}", crate::fl!("tui-help-global-unlock"))),
        Line::from(format!("  {}", crate::fl!("tui-help-global-lock"))),
        Line::from(format!("  {}", crate::fl!("tui-help-global-help"))),
        Line::from(format!("  {}", crate::fl!("tui-help-global-quit"))),
        Line::from(""),
        Line::from(Span::styled(
            crate::fl!("tui-help-accounts"),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            crate::fl!("tui-help-transactions"),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            crate::fl!("tui-help-logs"),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            crate::fl!("tui-help-close"),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", crate::fl!("tui-help-title")))
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_prompt(prompt: &super::app::Prompt, frame: &mut Frame<'_>) {
    let area = centered_rect(50, 20, frame.area());
    frame.render_widget(Clear, area);

    let display = if prompt.masked {
        "*".repeat(prompt.value.chars().count())
    } else {
        prompt.value.clone()
    };

    let lines = vec![
        Line::from(""),
        Line::from(Span::raw(format!("  {display}_"))),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", crate::fl!("tui-prompt-hint")),
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", prompt.title))
        .border_style(Style::default().fg(Color::Yellow));
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Returns a `Rect` centered in `area`, sized to a percentage of it.
pub(super) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
