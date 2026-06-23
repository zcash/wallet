//! Per-account balances view, with a `minconf` control.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};

use crate::commands::tui::app::App;

pub(crate) fn on_key(app: &mut App, key: KeyEvent) {
    match key.code {
        // Adjust minconf with +/- (and the unshifted '=' for convenience).
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.data.minconf = app.data.minconf.saturating_add(1);
        }
        KeyCode::Char('-') => {
            app.data.minconf = app.data.minconf.saturating_sub(1);
        }
        _ => {}
    }
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let header = Row::new(vec![
        Cell::from(crate::fl!("tui-bal-header-account")),
        Cell::from(crate::fl!("tui-bal-header-transparent")),
        Cell::from(crate::fl!("tui-bal-header-sapling")),
        Cell::from(crate::fl!("tui-bal-header-orchard")),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row<'_>> = match &app.data.balances {
        Some(balances) => balances
            .accounts
            .iter()
            .map(|acct| {
                let name = app
                    .data
                    .accounts
                    .iter()
                    .find(|a| a.account_uuid == acct.account_uuid)
                    .and_then(|a| a.name.clone())
                    .unwrap_or_else(|| short_uuid(&acct.account_uuid));
                Row::new(vec![
                    Cell::from(name),
                    Cell::from(acct.transparent.clone().unwrap_or_else(|| "0".into())),
                    Cell::from(acct.sapling.clone().unwrap_or_else(|| "0".into())),
                    Cell::from(acct.orchard.clone().unwrap_or_else(|| "0".into())),
                ])
            })
            .collect(),
        None => Vec::new(),
    };

    let title = format!(
        " {} ",
        crate::fl!("tui-bal-title", minconf = app.data.minconf)
    );

    if app.data.balances_syncing {
        let p = Paragraph::new(crate::fl!("tui-bal-syncing"))
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(p, area);
        return;
    }

    if rows.is_empty() {
        let p = Paragraph::new(crate::fl!("tui-bal-empty"))
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(p, area);
        return;
    }

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(34),
            Constraint::Percentage(22),
            Constraint::Percentage(22),
            Constraint::Percentage(22),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(table, area);
}

fn short_uuid(uuid: &str) -> String {
    if uuid.len() > 8 {
        uuid[..8].to_string()
    } else {
        uuid.to_string()
    }
}
