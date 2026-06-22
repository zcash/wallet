//! Transactions view (paginated list + detail pane).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::commands::tui::app::{App, TX_PAGE_SIZE};

pub(crate) async fn on_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            if app.tx_selected + 1 < app.data.transactions.len() {
                app.tx_selected += 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.tx_selected = app.tx_selected.saturating_sub(1);
        }
        KeyCode::Char(']') => {
            // Next page (only if the current page was full).
            if app.data.transactions.len() as u32 == TX_PAGE_SIZE {
                app.tx_offset += TX_PAGE_SIZE;
                app.tx_selected = 0;
                app.refresh_transactions().await;
            }
        }
        KeyCode::Char('[') => {
            if app.tx_offset >= TX_PAGE_SIZE {
                app.tx_offset -= TX_PAGE_SIZE;
            } else {
                app.tx_offset = 0;
            }
            app.tx_selected = 0;
            app.refresh_transactions().await;
        }
        _ => {}
    }
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(8)])
        .split(area);

    render_list(app, frame, chunks[0]);
    render_detail(app, frame, chunks[1]);
}

fn render_list(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let page = app.tx_offset / TX_PAGE_SIZE + 1;
    let title = format!(" Transactions  (page {page}, [ / ] to page · experimental) ");

    if app.data.transactions.is_empty() {
        let p = Paragraph::new("No transactions to display.")
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, area);
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .data
        .transactions
        .iter()
        .map(|tx| {
            let height = tx
                .mined_height
                .map(|h| h.to_string())
                .unwrap_or_else(|| "unmined".into());
            let delta = format_delta(tx.account_balance_delta);
            let delta_style = if tx.account_balance_delta >= 0 {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{height:>9}  "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{delta:>16}  "), delta_style),
                Span::raw(truncate(&tx.txid, 24)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(Some(app.tx_selected.min(app.data.transactions.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Detail ");

    let lines: Vec<Line<'_>> = match app.data.transactions.get(app.tx_selected) {
        Some(tx) => {
            let mut lines = vec![
                field("txid", tx.txid.clone()),
                field(
                    "height",
                    tx.mined_height
                        .map(|h| h.to_string())
                        .unwrap_or_else(|| "unmined".into()),
                ),
                field("delta", format_delta(tx.account_balance_delta)),
            ];
            if let Some(fee) = tx.fee_paid {
                lines.push(field("fee", format_zat(fee)));
            }
            if let Some(t) = tx.block_time {
                lines.push(field("block time", t.to_string()));
            }
            if let Some(uuid) = &tx.account_uuid {
                lines.push(field("account", uuid.clone()));
            }
            if tx.expired_unmined {
                lines.push(Line::from(Span::styled(
                    "expired (unmined)",
                    Style::default().fg(Color::Red),
                )));
            }
            lines
        }
        None => vec![Line::from(Span::styled(
            "Select a transaction.",
            Style::default().fg(Color::DarkGray),
        ))],
    };

    let p = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(p, area);
}

fn field(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:>11}: "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(value),
    ])
}

/// Formats a zatoshi-denominated balance delta as a signed ZEC string.
fn format_delta(zat: i64) -> String {
    let sign = if zat >= 0 { "+" } else { "-" };
    format!("{sign}{}", format_zec(zat.unsigned_abs()))
}

fn format_zat(zat: i64) -> String {
    format_zec(zat.unsigned_abs())
}

fn format_zec(zat: u64) -> String {
    let whole = zat / 100_000_000;
    let frac = zat % 100_000_000;
    format!("{whole}.{frac:08} ZEC")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}
