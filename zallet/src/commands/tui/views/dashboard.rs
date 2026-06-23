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
            lines.push(field(
                &crate::fl!("tui-dash-node-tip"),
                status.node_tip.height.to_string(),
            ));
            lines.push(field(
                &crate::fl!("tui-dash-node-hash"),
                shorten(&status.node_tip.blockhash),
            ));
            match &status.wallet_tip {
                Some(tip) => lines.push(field(
                    &crate::fl!("tui-dash-wallet-tip"),
                    tip.height.to_string(),
                )),
                None => lines.push(field(
                    &crate::fl!("tui-dash-wallet-tip"),
                    crate::fl!("tui-dash-not-syncing"),
                )),
            }
            match status.fully_synced_height {
                Some(h) => lines.push(field(
                    &crate::fl!("tui-dash-fully-synced-to"),
                    h.to_string(),
                )),
                None => lines.push(field(
                    &crate::fl!("tui-dash-fully-synced-to"),
                    "—".to_string(),
                )),
            }
        }
        None => lines.push(Line::from(Span::styled(
            crate::fl!("tui-dash-loading-status"),
            Style::default().fg(Color::DarkGray),
        ))),
    }

    lines.push(field(
        &crate::fl!("tui-dash-accounts"),
        app.data.accounts.len().to_string(),
    ));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", crate::fl!("tui-dash-status-title")));
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
                crate::fl!(
                    "tui-dash-sync-progress",
                    percent = format!("{:.1}", ratio * 100.0),
                    blocks = work.unscanned_blocks
                ),
            )
        }
        // No work remaining means fully synced (or no data yet).
        None => (1.0, crate::fl!("tui-dash-fully-synced")),
    };

    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", crate::fl!("tui-dash-sync-title"))),
        )
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(ratio)
        .label(label);
    frame.render_widget(gauge, area);
}

fn render_balances(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let mut lines = Vec::new();

    if app.data.balances_syncing {
        lines.push(Line::from(Span::styled(
            crate::fl!("tui-dash-balances-syncing"),
            Style::default().fg(Color::Yellow),
        )));
    } else {
        match &app.data.total_balance {
            Some(tb) => {
                lines.push(big_field(
                    &crate::fl!("tui-dash-total"),
                    &tb.total,
                    Color::Green,
                ));
                lines.push(field(
                    &crate::fl!("tui-dash-shielded"),
                    crate::fl!("tui-amount-zec", amount = tb.private.clone()),
                ));
                lines.push(field(
                    &crate::fl!("tui-dash-transparent"),
                    crate::fl!("tui-amount-zec", amount = tb.transparent.clone()),
                ));
            }
            None => lines.push(Line::from(Span::styled(
                crate::fl!("tui-dash-total-unavailable"),
                Style::default().fg(Color::DarkGray),
            ))),
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        crate::fl!("tui-dash-minconf", minconf = app.data.minconf),
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", crate::fl!("tui-dash-balance-title")));
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
            crate::fl!("tui-amount-zec", amount = value),
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
