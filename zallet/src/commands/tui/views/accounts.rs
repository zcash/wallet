//! Accounts view.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::commands::tui::app::App;
use crate::commands::tui::client::AccountBalance;

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
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", crate::fl!("tui-accounts-title")));
        let p = ratatui::widgets::Paragraph::new(crate::fl!("tui-accounts-empty"))
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, area);
        return;
    }

    // Build a per-account balance lookup once, keyed by account UUID, so rendering each row
    // is O(1) rather than scanning the full balance list per account.
    let balances: HashMap<&str, &AccountBalance> = app
        .data
        .balances
        .as_ref()
        .map(|b| {
            b.accounts
                .iter()
                .map(|a| (a.account_uuid.as_str(), a))
                .collect()
        })
        .unwrap_or_default();

    let items: Vec<ListItem<'_>> = app
        .data
        .accounts
        .iter()
        .map(|acct| {
            let name = acct
                .name
                .clone()
                .unwrap_or_else(|| crate::fl!("tui-value-unnamed"));
            let balance = account_balance(&balances, &acct.account_uuid);
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
                .title(format!(" {} ", crate::fl!("tui-accounts-title-list"))),
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
fn account_balance(balances: &HashMap<&str, &AccountBalance>, uuid: &str) -> String {
    let Some(acct) = balances.get(uuid) else {
        return String::new();
    };

    let mut parts = Vec::new();
    if let Some(t) = &acct.transparent {
        parts.push(crate::fl!(
            "tui-accounts-balance-transparent",
            amount = t.clone()
        ));
    }
    if let Some(s) = &acct.sapling {
        parts.push(crate::fl!(
            "tui-accounts-balance-sapling",
            amount = s.clone()
        ));
    }
    if let Some(o) = &acct.orchard {
        parts.push(crate::fl!(
            "tui-accounts-balance-orchard",
            amount = o.clone()
        ));
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
