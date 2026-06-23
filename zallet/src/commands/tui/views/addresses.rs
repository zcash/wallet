//! Addresses / receive view: per-account address list with QR rendering.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::commands::tui::{app::App, qr};

pub(crate) async fn on_key(app: &mut App, key: KeyEvent) {
    match key.code {
        // Account selector.
        KeyCode::Left | KeyCode::Char('h') => {
            select_prev_account(app);
            app.addresses_selected = 0;
        }
        KeyCode::Right | KeyCode::Char('l') => {
            select_next_account(app);
            app.addresses_selected = 0;
        }
        // Address selector within the account.
        KeyCode::Down | KeyCode::Char('j') => {
            let n = account_addresses(app).len();
            if app.addresses_selected + 1 < n {
                app.addresses_selected += 1;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.addresses_selected = app.addresses_selected.saturating_sub(1);
        }
        KeyCode::Char('a') => derive_new_address(app).await,
        _ => {}
    }
}

fn select_next_account(app: &mut App) {
    if !app.data.accounts.is_empty() {
        app.receive_account = (app.receive_account + 1) % app.data.accounts.len();
    }
}

fn select_prev_account(app: &mut App) {
    if !app.data.accounts.is_empty() {
        let n = app.data.accounts.len();
        app.receive_account = (app.receive_account + n - 1) % n;
    }
}

async fn derive_new_address(app: &mut App) {
    let Some(account) = app.data.accounts.get(app.receive_account) else {
        app.error(crate::fl!("tui-addr-no-account-selected"));
        return;
    };
    let uuid = account.account_uuid.clone();
    match app.client().new_address_for_account(&uuid).await {
        Ok(Ok(_)) => {
            app.info(crate::fl!("tui-addr-derived"));
            app.refresh().await;
        }
        Ok(Err(e)) if e.is_unlock_needed() => {
            app.error(crate::fl!("tui-err-locked-press-u-upper"));
        }
        Ok(Err(e)) => app.error(crate::fl!(
            "tui-err-rpc-call",
            method = "z_getaddressforaccount",
            error = e.to_string()
        )),
        Err(e) => app.error(e.to_string()),
    }
}

/// A single receiving address with a label for its kind.
struct AddressEntry {
    kind: String,
    address: String,
}

/// Collects the receiving addresses for the currently-selected account.
fn account_addresses(app: &App) -> Vec<AddressEntry> {
    let Some(account) = app.data.accounts.get(app.receive_account) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for addr in &account.addresses {
        if let Some(ua) = &addr.ua {
            out.push(AddressEntry {
                kind: crate::fl!("tui-addr-kind-unified"),
                address: ua.clone(),
            });
        }
        if let Some(sapling) = &addr.sapling {
            out.push(AddressEntry {
                kind: crate::fl!("tui-addr-kind-sapling"),
                address: sapling.clone(),
            });
        }
        if let Some(t) = &addr.transparent {
            out.push(AddressEntry {
                kind: crate::fl!("tui-addr-kind-transparent"),
                address: t.clone(),
            });
        }
    }
    out
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let addresses = account_addresses(app);

    // Reserve a one-line account selector at the top, then split the rest between the
    // address list and the detail/QR pane. The split direction adapts to the terminal
    // size: side-by-side when wide, stacked when narrow.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    render_account_selector(app, frame, chunks[0]);

    let body = chunks[1];
    // A QR plus borders needs a fair amount of width; only go side-by-side when there is
    // comfortably enough room for both a readable list and the detail pane.
    let side_by_side = body.width >= 72;

    let panes = if side_by_side {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(28), Constraint::Min(0)])
            .split(body)
    } else {
        // Stacked: a compact list on top, detail (with QR if it fits) below.
        let list_height = (addresses.len() as u16 + 2).min(body.height / 2).max(3);
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_height), Constraint::Min(0)])
            .split(body)
    };

    render_list(app, &addresses, frame, panes[0]);
    render_detail(app, &addresses, frame, panes[1]);
}

fn render_account_selector(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let label = match app.data.accounts.get(app.receive_account) {
        Some(account) => format!("◀ {} ▶", account.label()),
        None => crate::fl!("tui-addr-no-accounts"),
    };
    // Keep the hint short, and drop it entirely on very narrow terminals.
    let mut spans = vec![
        Span::styled(
            crate::fl!("tui-addr-account-label"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            label,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if area.width >= 56 {
        spans.push(Span::styled(
            format!("   {}", crate::fl!("tui-addr-account-hint")),
            Style::default().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_list(app: &App, addresses: &[AddressEntry], frame: &mut Frame<'_>, area: Rect) {
    if addresses.is_empty() {
        let p = Paragraph::new(crate::fl!("tui-addr-empty"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", crate::fl!("tui-addr-title"))),
            )
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true });
        frame.render_widget(p, area);
        return;
    }

    // The address text is truncated to whatever width is left after the kind column and
    // the list chrome, so the list never overflows a narrow pane.
    let avail = area.width.saturating_sub(2 + 2 + 13) as usize; // borders, marker, kind col
    let items: Vec<ListItem<'_>> = addresses
        .iter()
        .map(|entry| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<12}", entry.kind),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(truncate(&entry.address, avail.max(8))),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", crate::fl!("tui-addr-title"))),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    state.select(Some(app.addresses_selected.min(addresses.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Renders the detail pane for the selected address: its kind, the full address (wrapped
/// to the pane width), and a QR code if there is room to draw one.
fn render_detail(app: &App, addresses: &[AddressEntry], frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", crate::fl!("tui-addr-receive-title")));

    let Some(entry) = addresses.get(
        app.addresses_selected
            .min(addresses.len().saturating_sub(1)),
    ) else {
        let p = Paragraph::new(crate::fl!("tui-addr-select"))
            .block(block)
            .style(Style::default().fg(Color::DarkGray))
            .wrap(Wrap { trim: true });
        frame.render_widget(p, area);
        return;
    };

    // Interior dimensions (inside the border).
    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);

    let mut lines: Vec<Line<'_>> = Vec::new();

    // The address kind and full address text always shown (wrapped by the Paragraph).
    lines.push(Line::from(Span::styled(
        entry.kind.clone(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        entry.address.clone(),
        Style::default().fg(Color::Gray),
    )));
    lines.push(Line::from(""));

    // Render the QR only if it fits within the remaining interior space; otherwise show a
    // short hint so the pane stays usable on small terminals.
    let address_rows = ((entry.address.len() as u16) / inner_w.max(1)) + 1;
    let rows_left = inner_h.saturating_sub(2 + address_rows + 1);
    match qr::render(&entry.address) {
        Some(rendered) if rendered.width <= inner_w && rendered.height <= rows_left => {
            lines.extend(rendered.lines);
        }
        Some(_) => {
            lines.push(Line::from(Span::styled(
                crate::fl!("tui-addr-qr-enlarge"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        None => {
            lines.push(Line::from(Span::styled(
                crate::fl!("tui-addr-qr-too-long"),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let p = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(p, area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}
