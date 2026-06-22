//! Accounts view.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::commands::tui::app::App;

pub(crate) async fn on_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            if app.accounts_selected + 1 < app.data.accounts.len() {
                app.accounts_selected += 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.accounts_selected = app.accounts_selected.saturating_sub(1);
        }
        KeyCode::Char('n') => app.open_new_account_prompt(),
        _ => {}
    }
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    if app.data.accounts.is_empty() {
        let block = Block::default().borders(Borders::ALL).title(" Accounts ");
        let p = ratatui::widgets::Paragraph::new(
            "No accounts yet. Press 'n' to create one (wallet must be unlocked).",
        )
        .block(block)
        .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, area);
        return;
    }

    // Build a per-account balance lookup.
    let items: Vec<ListItem<'_>> = app
        .data
        .accounts
        .iter()
        .map(|acct| {
            let name = acct.name.clone().unwrap_or_else(|| "(unnamed)".into());
            let balance = account_balance(app, &acct.account_uuid);
            let line = Line::from(vec![
                Span::styled(
                    format!("{name:<24}"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}  ", short_uuid(&acct.account_uuid)),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(balance, Style::default().fg(Color::Green)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Accounts  ([n]ew) "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(Some(app.accounts_selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Sums the per-pool balances for an account into a display string.
fn account_balance(app: &App, uuid: &str) -> String {
    let Some(balances) = &app.data.balances else {
        return String::new();
    };
    let Some(acct) = balances.accounts.iter().find(|a| a.account_uuid == uuid) else {
        return String::new();
    };

    let mut parts = Vec::new();
    if let Some(t) = &acct.transparent {
        parts.push(format!("t:{t}"));
    }
    if let Some(s) = &acct.sapling {
        parts.push(format!("s:{s}"));
    }
    if let Some(o) = &acct.orchard {
        parts.push(format!("o:{o}"));
    }
    parts.join("  ")
}

fn short_uuid(uuid: &str) -> String {
    if uuid.len() > 8 {
        uuid[..8].to_string()
    } else {
        uuid.to_string()
    }
}
