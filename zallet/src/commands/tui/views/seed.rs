//! Seed view: reveals the BIP 39 mnemonic phrase for an account, behind a confirmation.
//!
//! SECURITY: The mnemonic phrase is the wallet's most sensitive secret. This view requires
//! an explicit confirmation before revealing it, and offers a way to hide it again.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::commands::tui::app::App;

pub(crate) async fn on_key(app: &mut App, key: KeyEvent) {
    // Confirmation prompt for revealing the phrase.
    if app.seed.confirming {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.seed.confirming = false;
                app.reveal_seed().await;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                app.seed.confirming = false;
                app.info("Cancelled");
            }
            _ => {}
        }
        return;
    }

    match key.code {
        // Account selector.
        KeyCode::Left | KeyCode::Char('h') => {
            select_prev_account(app);
            hide(app);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            select_next_account(app);
            hide(app);
        }
        // Reveal (asks for confirmation first) or hide.
        KeyCode::Enter | KeyCode::Char('r') => {
            if app.seed.revealed.is_some() {
                hide(app);
                app.info("Seed phrase hidden");
            } else if app.data.accounts.is_empty() {
                app.error("No accounts available.");
            } else {
                app.seed.confirming = true;
            }
        }
        KeyCode::Char('c') => {
            hide(app);
            app.info("Seed phrase hidden");
        }
        _ => {}
    }
}

fn hide(app: &mut App) {
    app.seed.revealed = None;
    app.seed.revealed_seedfp = None;
}

fn select_next_account(app: &mut App) {
    if !app.data.accounts.is_empty() {
        app.seed.account = (app.seed.account + 1) % app.data.accounts.len();
    }
}

fn select_prev_account(app: &mut App) {
    if !app.data.accounts.is_empty() {
        let n = app.data.accounts.len();
        app.seed.account = (app.seed.account + n - 1) % n;
    }
}

pub(crate) fn render(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    render_account_selector(app, frame, chunks[0]);
    render_body(app, frame, chunks[1]);
}

fn render_account_selector(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let label = match app.data.accounts.get(app.seed.account) {
        Some(account) => format!("◀ {} ▶", account.label()),
        None => "(no accounts)".to_string(),
    };
    let line = Line::from(vec![
        Span::styled(" Account: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            label,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   (←/→ switch account)",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_body(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Seed phrase ");

    let lines: Vec<Line<'_>> = if app.seed.confirming {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Reveal the seed phrase for this account?",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("  Anyone who sees this phrase can steal all funds derived from it"),
            Line::from("  and recover your full transaction history. Make sure no one is"),
            Line::from("  watching your screen."),
            Line::from(""),
            Line::from(Span::styled(
                "  [y] reveal    [n] cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else if let Some(phrase) = &app.seed.revealed {
        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  ⚠ SECRET — anyone with this phrase can spend your funds.",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        // Render the words in a numbered grid for legibility.
        for line in numbered_words(phrase) {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        if let Some(seedfp) = &app.seed.revealed_seedfp {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  seed fingerprint: {seedfp}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  [c] or Enter to hide",
            Style::default().fg(Color::DarkGray),
        )));
        lines
    } else {
        vec![
            Line::from(""),
            Line::from("  The seed phrase lets you recover this wallet's funds."),
            Line::from("  Keep it secret and back it up offline."),
            Line::from(""),
            Line::from(Span::styled(
                "  Press Enter (or 'r') to reveal the phrase for the selected account.",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                "  The wallet must be unlocked.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Splits a mnemonic into rows of numbered words, e.g. `" 1. word   2. word   3. word"`.
fn numbered_words(phrase: &str) -> Vec<String> {
    use std::fmt::Write;

    let words: Vec<&str> = phrase.split_whitespace().collect();
    words
        .chunks(4)
        .enumerate()
        .map(|(row, chunk)| {
            let mut line = String::new();
            for (col, w) in chunk.iter().enumerate() {
                let _ = write!(line, "{:>2}. {:<12}", row * 4 + col + 1, w);
            }
            line
        })
        .collect()
}
