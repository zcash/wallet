//! Send view: a form for `z_sendmany` with confirmation and inline operation polling.
//!
//! The "from" field is an account selector rather than free text: `z_sendmany` requires an
//! address as its source, so the selected account is resolved to one of its addresses (a
//! unified address where possible) at submit time.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::commands::tui::app::{App, PRIVACY_POLICIES, SendField};

/// Handles a key event for the send view.
///
/// View navigation (`Esc`, `Tab`, `BackTab`) is handled by the caller unless a text field
/// is being edited, in which case all keys are routed here.
pub(crate) async fn on_key(app: &mut App, key: KeyEvent) {
    // While confirming, only y/n are meaningful.
    if app.send.confirming {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.send.confirming = false;
                submit(app).await;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.send.confirming = false;
                app.info("Send cancelled");
            }
            _ => {}
        }
        return;
    }

    // Editing mode: keystrokes go into the focused text field until Esc/Enter leaves it.
    if app.send.editing {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => app.send.editing = false,
            KeyCode::Backspace => {
                if let Some(buf) = text_field_mut(app) {
                    buf.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(buf) = text_field_mut(app) {
                    buf.push(c);
                }
            }
            _ => {}
        }
        return;
    }

    // Navigation mode: keys move between fields and operate selectors.
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => app.send.field = next_field(app.send.field),
        KeyCode::Up | KeyCode::Char('k') => app.send.field = prev_field(app.send.field),

        // Selectors respond to left/right (and h/l).
        KeyCode::Left | KeyCode::Char('h') if app.send.field == SendField::From => {
            select_prev_account(app)
        }
        KeyCode::Right | KeyCode::Char('l') if app.send.field == SendField::From => {
            select_next_account(app)
        }
        KeyCode::Left | KeyCode::Char('h') if app.send.field == SendField::PrivacyPolicy => {
            app.send.privacy_policy = app.send.privacy_policy.saturating_sub(1);
        }
        KeyCode::Right | KeyCode::Char('l') if app.send.field == SendField::PrivacyPolicy => {
            app.send.privacy_policy = (app.send.privacy_policy + 1).min(PRIVACY_POLICIES.len() - 1);
        }

        // `Enter` and `i` (vim-style insert) both begin editing a text field. On the
        // Submit row, `Enter` reviews & sends.
        KeyCode::Char('i') if app.send.field.is_text() => {
            app.send.editing = true;
        }
        KeyCode::Enter => {
            if app.send.field.is_text() {
                // Begin editing this text field.
                app.send.editing = true;
            } else if app.send.field == SendField::Submit {
                // Review & send.
                if let Err(msg) = validate(app) {
                    app.error(msg);
                } else {
                    app.send.confirming = true;
                }
            }
        }
        _ => {}
    }
}

fn select_next_account(app: &mut App) {
    if !app.data.accounts.is_empty() {
        app.send.from_account = (app.send.from_account + 1) % app.data.accounts.len();
    }
}

fn select_prev_account(app: &mut App) {
    if !app.data.accounts.is_empty() {
        let n = app.data.accounts.len();
        app.send.from_account = (app.send.from_account + n - 1) % n;
    }
}

fn validate(app: &App) -> Result<(), String> {
    if app.data.accounts.is_empty() {
        return Err("No accounts available to send from".into());
    }
    let account = app
        .data
        .accounts
        .get(app.send.from_account)
        .ok_or("Select a source account")?;
    if account.spend_source_address().is_none() {
        return Err("Selected account has no spendable address".into());
    }
    if app.send.to.trim().is_empty() {
        return Err("Recipient address is required".into());
    }
    if app.send.amount.trim().is_empty() {
        return Err("Amount is required".into());
    }
    if app.send.amount.trim().parse::<f64>().is_err() {
        return Err("Amount must be a number".into());
    }
    Ok(())
}

async fn submit(app: &mut App) {
    let Some(account) = app.data.accounts.get(app.send.from_account) else {
        app.error("No source account selected");
        return;
    };
    let Some(from) = account.spend_source_address().map(|s| s.to_string()) else {
        app.error("Selected account has no spendable address");
        return;
    };

    let to = app.send.to.trim().to_string();
    let amount = app.send.amount.trim().to_string();
    let memo = {
        let m = app.send.memo.trim();
        if m.is_empty() {
            None
        } else {
            Some(m.to_string())
        }
    };
    let policy = PRIVACY_POLICIES[app.send.privacy_policy];

    match app
        .client()
        .send_many(&from, &to, &amount, memo.as_deref(), policy)
        .await
    {
        Ok(Ok(opid)) => {
            app.info(format!("Submitted (op {opid})"));
            app.send.pending_opid = Some(opid);
            app.send.pending_status = None;
            app.poll_send().await;
        }
        Ok(Err(e)) if e.is_unlock_needed() => {
            app.error("Wallet is locked. Press 'U' to unlock first.");
        }
        Ok(Err(e)) => app.error(format!("z_sendmany: {e}")),
        Err(e) => app.error(e.to_string()),
    }
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(6)])
        .split(area);

    render_form(app, frame, chunks[0]);
    render_status(app, frame, chunks[1]);
}

fn render_form(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let f = &app.send;
    let policy = PRIVACY_POLICIES[f.privacy_policy];

    // The "from" account selector.
    let from_label = match app.data.accounts.get(f.from_account) {
        Some(account) => {
            let mut s = format!("◀ {} ▶", account.label());
            if account.spend_source_address().is_none() {
                s.push_str("  (no spendable address)");
            }
            s
        }
        None => "(no accounts)".to_string(),
    };

    let mut lines = vec![
        Line::from(vec![
            label_span("From", f.field == SendField::From),
            Span::styled(from_label, Style::default().fg(Color::Cyan)),
        ]),
        input_line("To", &f.to, f.field == SendField::To, f.editing),
        input_line(
            "Amount (ZEC)",
            &f.amount,
            f.field == SendField::Amount,
            f.editing,
        ),
        input_line("Memo", &f.memo, f.field == SendField::Memo, f.editing),
        Line::from(vec![
            label_span("Privacy policy", f.field == SendField::PrivacyPolicy),
            Span::styled(format!("◀ {policy} ▶"), policy_style(f.privacy_policy)),
        ]),
        Line::from(""),
        // The "Review & send" action row.
        Line::from(Span::styled(
            "  [ Review & send ]",
            if f.field == SendField::Submit {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            },
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Fees are computed automatically (ZIP-317).",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    if f.privacy_policy > 0 {
        lines.push(Line::from(Span::styled(
            "⚠ This policy reduces privacy. Only proceed if you understand the implications.",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
    }

    lines.push(Line::from(""));
    let hint = if f.editing {
        "EDITING — type to enter text · Enter/Esc to finish"
    } else if f.field.is_text() {
        "↑↓ move · Enter to edit this field · Esc to tabs"
    } else if f.field == SendField::Submit {
        "↑↓ move · Enter to review & send · Esc to tabs"
    } else {
        "↑↓ move · ←/→ change selection · Esc to tabs"
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(Color::DarkGray),
    )));

    let block = Block::default().borders(Borders::ALL).title(" Send ");
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_status(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Operation ");

    let lines: Vec<Line<'_>> = if app.send.confirming {
        let f = &app.send;
        let from = app
            .data
            .accounts
            .get(f.from_account)
            .map(|a| a.label())
            .unwrap_or_default();
        vec![
            Line::from(Span::styled(
                "Confirm send?",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(
                "  {} ZEC from {from} → {}",
                f.amount.trim(),
                f.to.trim()
            )),
            Line::from(Span::styled(
                "  [y] yes   [n] no",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else if let Some(opid) = &app.send.pending_opid {
        let status = app
            .send
            .pending_status
            .as_ref()
            .map(|s| s.status.clone())
            .unwrap_or_else(|| "queued".into());
        vec![
            Line::from(format!("Operation {opid}")),
            Line::from(Span::styled(
                format!("Status: {status}…"),
                Style::default().fg(Color::Cyan),
            )),
        ]
    } else if let Some(status) = &app.send.pending_status {
        // Finished op; show the result/error.
        match status.status.as_str() {
            "success" => {
                let txid = status
                    .result
                    .as_ref()
                    .and_then(|r| r.get("txid"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("(unknown)");
                vec![
                    Line::from(Span::styled(
                        "Send succeeded",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(format!("txid: {txid}")),
                ]
            }
            _ => vec![Line::from(Span::styled(
                "Last send did not succeed (see footer).",
                Style::default().fg(Color::Red),
            ))],
        }
    } else {
        vec![Line::from(Span::styled(
            "Fill in the form and press Enter to review.",
            Style::default().fg(Color::DarkGray),
        ))]
    };

    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn input_line<'a>(label: &'a str, value: &'a str, focused: bool, editing: bool) -> Line<'a> {
    // Only show the text cursor when actively editing this (focused) field.
    let cursor = if focused && editing { "_" } else { "" };
    let value_style = if focused && editing {
        Style::default().fg(Color::White)
    } else {
        Style::default()
    };
    Line::from(vec![
        label_span(label, focused),
        Span::styled(format!("{value}{cursor}"), value_style),
    ])
}

fn label_span(label: &str, focused: bool) -> Span<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Span::styled(format!(" {label:>14}: "), style)
}

fn policy_style(index: usize) -> Style {
    if index == 0 {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Yellow)
    }
}

fn next_field(field: SendField) -> SendField {
    match field {
        SendField::From => SendField::To,
        SendField::To => SendField::Amount,
        SendField::Amount => SendField::Memo,
        SendField::Memo => SendField::PrivacyPolicy,
        SendField::PrivacyPolicy => SendField::Submit,
        SendField::Submit => SendField::From,
    }
}

fn prev_field(field: SendField) -> SendField {
    match field {
        SendField::From => SendField::Submit,
        SendField::To => SendField::From,
        SendField::Amount => SendField::To,
        SendField::Memo => SendField::Amount,
        SendField::PrivacyPolicy => SendField::Memo,
        SendField::Submit => SendField::PrivacyPolicy,
    }
}

/// Returns the editable text buffer for the currently-focused field, or `None` if the
/// focused field is a selector rather than a text field.
fn text_field_mut(app: &mut App) -> Option<&mut String> {
    match app.send.field {
        SendField::To => Some(&mut app.send.to),
        SendField::Amount => Some(&mut app.send.amount),
        SendField::Memo => Some(&mut app.send.memo),
        SendField::From | SendField::PrivacyPolicy | SendField::Submit => None,
    }
}
