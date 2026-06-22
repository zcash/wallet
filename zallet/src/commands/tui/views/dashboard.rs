//! Dashboard / status view.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
};

use crate::commands::tui::app::App;

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // status
            Constraint::Length(3), // sync gauge
            Constraint::Min(0),    // balances
        ])
        .split(area);

    render_status(app, frame, chunks[0]);
    render_sync(app, frame, chunks[1]);
    render_balances(app, frame, chunks[2]);
}

fn render_status(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let mut lines = Vec::new();

    match &app.data.status {
        Some(status) => {
            lines.push(field("Node tip", status.node_tip.height.to_string()));
            lines.push(field("Node hash", shorten(&status.node_tip.blockhash)));
            match &status.wallet_tip {
                Some(tip) => lines.push(field("Wallet tip", tip.height.to_string())),
                None => lines.push(field("Wallet tip", "(not yet syncing)".to_string())),
            }
            match status.fully_synced_height {
                Some(h) => lines.push(field("Fully synced to", h.to_string())),
                None => lines.push(field("Fully synced to", "—".to_string())),
            }
        }
        None => lines.push(Line::from(Span::styled(
            "Loading wallet status…",
            Style::default().fg(Color::DarkGray),
        ))),
    }

    lines.push(field("Accounts", app.data.accounts.len().to_string()));

    let block = Block::default().borders(Borders::ALL).title(" Status ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_sync(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let (ratio, label) = match app
        .data
        .status
        .as_ref()
        .and_then(|s| s.sync_work_remaining.as_ref())
    {
        Some(work) => {
            let p = &work.progress;
            let ratio = if p.denominator == 0 {
                1.0
            } else {
                (p.numerator as f64 / p.denominator as f64).clamp(0.0, 1.0)
            };
            (
                ratio,
                format!(
                    "{:.1}%  ({} blocks remaining)",
                    ratio * 100.0,
                    work.unscanned_blocks
                ),
            )
        }
        // No work remaining means fully synced (or no data yet).
        None => (1.0, "Fully synced".to_string()),
    };

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Sync "))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(ratio)
        .label(label);
    frame.render_widget(gauge, area);
}

fn render_balances(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let mut lines = Vec::new();

    if app.data.balances_syncing {
        lines.push(Line::from(Span::styled(
            "Balances are not available yet — the wallet is still syncing.",
            Style::default().fg(Color::Yellow),
        )));
    } else {
        match &app.data.total_balance {
            Some(tb) => {
                lines.push(big_field("Total", &tb.total, Color::Green));
                lines.push(field("Shielded (private)", format!("{} ZEC", tb.private)));
                lines.push(field("Transparent", format!("{} ZEC", tb.transparent)));
            }
            None => lines.push(Line::from(Span::styled(
                "Total balance unavailable (watch-only, or not yet synced).",
                Style::default().fg(Color::DarkGray),
            ))),
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("minconf = {}", app.data.minconf),
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default().borders(Borders::ALL).title(" Balance ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:>18}: "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value),
    ])
}

fn big_field(label: &str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:>18}: "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{value} ZEC"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn shorten(hash: &str) -> String {
    if hash.len() > 20 {
        format!("{}…{}", &hash[..10], &hash[hash.len() - 8..])
    } else {
        hash.to_string()
    }
}
