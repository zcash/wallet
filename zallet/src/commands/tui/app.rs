//! TUI application state and event loop.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::error::Error;

use super::client::{
    Account, Balances, LockState, OperationStatus, TotalBalance, WalletClient, WalletStatus,
    WalletTx,
};
use super::event::{Event, EventSource};
use super::terminal::Tui;
use super::{ui, views};

/// How often the UI refreshes wallet data and re-renders.
const TICK_RATE: Duration = Duration::from_secs(3);

/// The number of transactions fetched per page in the transactions view.
pub(super) const TX_PAGE_SIZE: u32 = 50;

/// The primary views of the TUI.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    Dashboard,
    Accounts,
    Balances,
    Addresses,
    Transactions,
    Send,
    Seed,
    Logs,
}

impl View {
    /// All views, in tab order.
    pub(super) const ALL: [View; 8] = [
        View::Dashboard,
        View::Accounts,
        View::Balances,
        View::Addresses,
        View::Transactions,
        View::Send,
        View::Seed,
        View::Logs,
    ];

    pub(super) fn title(self) -> &'static str {
        match self {
            View::Dashboard => "Dashboard",
            View::Accounts => "Accounts",
            View::Balances => "Balances",
            View::Addresses => "Receive",
            View::Transactions => "Transactions",
            View::Send => "Send",
            View::Seed => "Seed",
            View::Logs => "Logs",
        }
    }

    fn index(self) -> usize {
        View::ALL.iter().position(|&v| v == self).unwrap_or(0)
    }
}

/// A compact summary of wallet sync progress, derived from `getwalletstatus`.
pub(super) struct SyncSummary {
    /// Fraction in `[0.0, 1.0]`, or `None` if indeterminate.
    pub(super) fraction: Option<f64>,
    /// Whether the wallet is fully synced.
    pub(super) synced: bool,
    /// The height the wallet is fully synced to, if known.
    pub(super) synced_height: Option<u32>,
    /// The node's chain tip height.
    pub(super) node_height: Option<u32>,
    /// Blocks still to scan, if known.
    pub(super) unscanned_blocks: Option<u32>,
}

impl SyncSummary {
    /// A short one-line label, e.g. `Sync 87%` or `Synced`.
    pub(super) fn short_label(&self) -> String {
        if self.synced {
            "Synced".to_string()
        } else if let Some(f) = self.fraction {
            format!("Sync {:.0}%", f * 100.0)
        } else {
            "Syncing…".to_string()
        }
    }
}

/// Where keyboard focus currently sits.
///
/// `Tabs` focus means navigation keys move between views (the header row is highlighted);
/// `View` focus means keys are handled by the active view's body. `Esc` moves focus from
/// the body up to the tabs; selecting/entering a view moves focus back down into it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Tabs,
    View,
}

/// A transient status message shown in the footer.
#[derive(Clone)]
pub(super) struct Toast {
    pub(super) text: String,
    pub(super) is_error: bool,
}

/// Cached wallet data, refreshed on each tick and after actions.
#[derive(Default)]
pub(super) struct WalletData {
    pub(super) status: Option<WalletStatus>,
    pub(super) total_balance: Option<TotalBalance>,
    pub(super) balances: Option<Balances>,
    pub(super) accounts: Vec<Account>,
    pub(super) transactions: Vec<WalletTx>,
    /// The minimum number of confirmations used when querying balances.
    pub(super) minconf: u32,
    /// Whether the wallet summary is ready yet (balances unavailable while syncing).
    pub(super) balances_syncing: bool,
}

/// The interactive send form.
#[derive(Default)]
pub(super) struct SendForm {
    /// The source account, as an index into [`WalletData::accounts`].
    pub(super) from_account: usize,
    pub(super) to: String,
    pub(super) amount: String,
    pub(super) memo: String,
    /// Index into [`PRIVACY_POLICIES`].
    pub(super) privacy_policy: usize,
    /// Which field currently has focus.
    pub(super) field: SendField,
    /// Whether the focused text field is currently being edited.
    ///
    /// Text fields only capture keystrokes while editing; otherwise navigation keys
    /// (`j`/`k`) move between fields. Editing is entered with `Enter` and left with `Esc`.
    pub(super) editing: bool,
    /// A submitted operation that is being polled, if any.
    pub(super) pending_opid: Option<String>,
    /// The latest known status of the pending operation.
    pub(super) pending_status: Option<OperationStatus>,
    /// Whether the confirmation prompt is showing.
    pub(super) confirming: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SendField {
    #[default]
    From,
    To,
    Amount,
    Memo,
    PrivacyPolicy,
    /// The "Review & send" action row.
    Submit,
}

impl SendField {
    /// Whether this field accepts free-text input (vs. a selector or action).
    pub(super) fn is_text(self) -> bool {
        matches!(self, SendField::To | SendField::Amount | SendField::Memo)
    }
}

/// The privacy policies offered in the send form, weakest-last.
pub(super) const PRIVACY_POLICIES: [&str; 7] = [
    "FullPrivacy",
    "AllowRevealedAmounts",
    "AllowRevealedRecipients",
    "AllowRevealedSenders",
    "AllowLinkingAccountAddresses",
    "AllowFullyTransparent",
    "NoPrivacy",
];

/// State for the Seed (mnemonic) view.
#[derive(Default)]
pub(super) struct SeedState {
    /// The account whose seed is selected, as an index into the accounts list.
    pub(super) account: usize,
    /// The revealed mnemonic phrase, if currently shown.
    pub(super) revealed: Option<String>,
    /// The seed fingerprint of the revealed phrase.
    pub(super) revealed_seedfp: Option<String>,
    /// Whether a reveal confirmation prompt is showing.
    pub(super) confirming: bool,
}

/// State for the Logs view.
#[derive(Default)]
pub(super) struct LogsState {
    /// The path to the log file, if known (self-hosted mode only).
    pub(super) path: Option<std::path::PathBuf>,
    /// The most recently loaded tail of the log file.
    pub(super) lines: Vec<String>,
    /// Scroll offset from the bottom, in lines (0 = following the tail).
    pub(super) scroll_from_bottom: usize,
    /// An error encountered while reading the log file, if any.
    pub(super) read_error: Option<String>,
}

/// A modal prompt for textual input (e.g. unlock passphrase, new account name).
pub(super) struct Prompt {
    pub(super) title: String,
    pub(super) value: String,
    pub(super) masked: bool,
    pub(super) kind: PromptKind,
}

#[derive(Clone, Copy)]
pub(super) enum PromptKind {
    Unlock,
    NewAccount,
}

/// The top-level application state.
pub(super) struct App {
    client: WalletClient,
    pub(super) view: View,
    pub(super) focus: Focus,
    pub(super) data: WalletData,
    pub(super) send: SendForm,
    pub(super) seed: SeedState,
    pub(super) logs: LogsState,
    pub(super) toast: Option<Toast>,
    pub(super) prompt: Option<Prompt>,
    pub(super) show_help: bool,
    /// The wallet's encryption/lock state, as last observed.
    pub(super) lock_state: LockState,
    /// Selection index for list-based views.
    pub(super) accounts_selected: usize,
    /// The account selected in the Receive view, as an index into the accounts list.
    pub(super) receive_account: usize,
    pub(super) addresses_selected: usize,
    pub(super) tx_selected: usize,
    pub(super) tx_offset: u32,
    should_quit: bool,
}

impl App {
    pub(super) fn new(client: WalletClient, log_path: Option<std::path::PathBuf>) -> Self {
        Self {
            client,
            view: View::Dashboard,
            focus: Focus::View,
            data: WalletData {
                minconf: 1,
                ..Default::default()
            },
            send: SendForm::default(),
            seed: SeedState::default(),
            logs: LogsState {
                path: log_path,
                ..Default::default()
            },
            toast: None,
            prompt: None,
            show_help: false,
            // Assume locked until we learn otherwise, so the UI never appears usable
            // before we have confirmed the wallet is accessible.
            lock_state: LockState::Locked,
            accounts_selected: 0,
            receive_account: 0,
            addresses_selected: 0,
            tx_selected: 0,
            tx_offset: 0,
            should_quit: false,
        }
    }

    /// Whether the wallet is currently inaccessible and must be unlocked before use.
    pub(super) fn is_gated(&self) -> bool {
        self.lock_state == LockState::Locked
    }

    /// Whether a text field is currently capturing keystrokes, in which case global
    /// keyboard shortcuts must be suppressed so the characters can be typed.
    fn is_text_input_active(&self) -> bool {
        self.view == View::Send && self.send.editing
    }

    /// Computes a summary of wallet sync progress from the latest `getwalletstatus`.
    ///
    /// Progress is wallet-wide (the backend does not expose per-account progress).
    pub(super) fn sync_summary(&self) -> SyncSummary {
        let Some(status) = &self.data.status else {
            return SyncSummary {
                fraction: None,
                synced: false,
                synced_height: None,
                node_height: None,
                unscanned_blocks: None,
            };
        };

        match &status.sync_work_remaining {
            // No work remaining means fully synced.
            None => SyncSummary {
                fraction: Some(1.0),
                synced: true,
                synced_height: status.fully_synced_height,
                node_height: Some(status.node_tip.height),
                unscanned_blocks: Some(0),
            },
            Some(work) => {
                let p = &work.progress;
                // The denominator can be zero when a range contains no shielded notes.
                let fraction = (p.denominator != 0)
                    .then(|| (p.numerator as f64 / p.denominator as f64).clamp(0.0, 1.0));
                SyncSummary {
                    fraction,
                    synced: false,
                    synced_height: status.fully_synced_height,
                    node_height: Some(status.node_tip.height),
                    unscanned_blocks: Some(work.unscanned_blocks),
                }
            }
        }
    }

    /// Runs the event loop until the user quits.
    pub(super) async fn run(&mut self, terminal: &mut Tui) -> Result<(), Error> {
        let mut events = EventSource::new(TICK_RATE);

        // Determine the wallet's lock state before doing anything else, so the UI never
        // appears usable when the wallet is locked.
        self.refresh_lock_state().await;
        if !self.is_gated() {
            self.refresh().await;
        }

        loop {
            terminal
                .draw(|frame| ui::render(self, frame))
                .map_err(|e| crate::error::ErrorKind::Generic.context(e))?;

            match events.next().await {
                Event::Key(key) => self.on_key(key).await,
                Event::Tick => self.on_tick().await,
                Event::Resize => {}
            }

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    /// Refreshes the wallet's encryption/lock state from `getwalletinfo`.
    pub(super) async fn refresh_lock_state(&mut self) {
        match self.client.get_wallet_info().await {
            Ok(Ok(info)) => self.lock_state = info.lock_state(),
            // If we can't determine the state, remain conservative: stay locked.
            Ok(Err(e)) => self.error(format!("getwalletinfo: {e}")),
            Err(e) => self.error(e.to_string()),
        }
    }

    /// Returns a reference to the wallet client.
    pub(super) fn client(&self) -> &WalletClient {
        &self.client
    }

    /// Shows a transient informational message.
    pub(super) fn info(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            is_error: false,
        });
    }

    /// Shows a transient error message.
    pub(super) fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            is_error: true,
        });
    }

    async fn on_tick(&mut self) {
        // The Logs view is available regardless of lock state, and follows the log tail.
        if self.view == View::Logs && self.logs.scroll_from_bottom == 0 {
            self.load_logs();
        }

        // Keep the lock state current (it can change when the unlock timeout elapses).
        self.refresh_lock_state().await;
        if self.is_gated() {
            // While locked, do not fetch or display any wallet data.
            return;
        }
        self.refresh().await;
        // Poll a pending send operation if there is one.
        if self.send.pending_opid.is_some() {
            self.poll_send().await;
        }
    }

    /// Refreshes cached wallet data from the backend.
    pub(super) async fn refresh(&mut self) {
        let minconf = self.data.minconf;

        match self.client.get_wallet_status().await {
            Ok(Ok(status)) => self.data.status = Some(status),
            Ok(Err(e)) => self.error(format!("getwalletstatus: {e}")),
            Err(e) => self.error(e.to_string()),
        }

        match self.client.get_total_balance(minconf).await {
            Ok(Ok(tb)) => self.data.total_balance = Some(tb),
            Ok(Err(_)) => {} // tolerated; total balance has stricter requirements
            Err(e) => self.error(e.to_string()),
        }

        match self.client.get_balances(minconf).await {
            Ok(Ok(b)) => {
                self.data.balances = Some(b);
                self.data.balances_syncing = false;
            }
            // `-28` (InWarmup) means the wallet summary isn't ready yet because the wallet
            // is still syncing/scanning. This is expected, not an error: surface it as a
            // "syncing" state rather than a scary toast.
            Ok(Err(e)) if e.code == -28 => {
                self.data.balances = None;
                self.data.balances_syncing = true;
            }
            Ok(Err(e)) => self.error(format!("z_getbalances: {e}")),
            Err(e) => self.error(e.to_string()),
        }

        match self.client.list_accounts().await {
            Ok(Ok(accounts)) => {
                self.data.accounts = accounts;
                self.clamp_selection();
            }
            Ok(Err(e)) => self.error(format!("z_listaccounts: {e}")),
            Err(e) => self.error(e.to_string()),
        }

        self.refresh_transactions().await;
    }

    pub(super) async fn refresh_transactions(&mut self) {
        match self
            .client
            .list_transactions(self.tx_offset, TX_PAGE_SIZE)
            .await
        {
            Ok(Ok(txs)) => {
                self.data.transactions = txs;
                if self.tx_selected >= self.data.transactions.len() {
                    self.tx_selected = self.data.transactions.len().saturating_sub(1);
                }
            }
            // z_listtransactions is experimental; don't spam errors on empty wallets.
            Ok(Err(_)) => self.data.transactions.clear(),
            Err(e) => self.error(e.to_string()),
        }
    }

    fn clamp_selection(&mut self) {
        if self.accounts_selected >= self.data.accounts.len() {
            self.accounts_selected = self.data.accounts.len().saturating_sub(1);
        }
    }

    async fn on_key(&mut self, key: KeyEvent) {
        // Global: Ctrl-C always quits.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        // A modal prompt takes precedence over everything else.
        if self.prompt.is_some() {
            self.on_key_prompt(key).await;
            return;
        }

        // The help overlay swallows input; any key dismisses it.
        if self.show_help {
            self.show_help = false;
            return;
        }

        // While the wallet is locked, the only permitted actions are unlocking, quitting,
        // viewing help, and viewing the (non-sensitive) logs. The wallet's data must not
        // appear usable.
        if self.is_gated() {
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('?') => self.show_help = true,
                KeyCode::Char('u') => self.open_unlock_prompt(),
                // Allow toggling to/from the Logs view while locked.
                KeyCode::Char('8') => self.set_view(View::Logs),
                KeyCode::Char('1') => self.set_view(View::Dashboard),
                // Enter unlocks unless we're on the Logs view (where it's not an action).
                KeyCode::Enter if self.view != View::Logs => self.open_unlock_prompt(),
                // Let the Logs view handle scrolling keys while locked.
                _ if self.view == View::Logs => views::logs::on_key(self, key),
                _ => {}
            }
            return;
        }

        // When a text field is actively being edited, all keystrokes must go to the field
        // (so e.g. '?', 'q', and digits are typed rather than triggering shortcuts).
        if self.is_text_input_active() {
            self.on_key_view(key).await;
            return;
        }

        // Keys handled regardless of focus.
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                return;
            }
            KeyCode::Char('r') => {
                self.refresh().await;
                self.info("Refreshed");
                return;
            }
            // Direct view jumps work from anywhere and focus the view body.
            KeyCode::Char(c @ '1'..='8') => {
                let idx = (c as u8 - b'1') as usize;
                if let Some(&v) = View::ALL.get(idx) {
                    self.set_view(v);
                    self.focus = Focus::View;
                }
                return;
            }
            // Lock/unlock shortcuts (only meaningful for encrypted wallets).
            KeyCode::Char('L') => {
                self.lock_wallet().await;
                return;
            }
            KeyCode::Char('U') => {
                self.open_unlock_prompt();
                return;
            }
            _ => {}
        }

        match self.focus {
            Focus::Tabs => self.on_key_tabs(key),
            Focus::View => self.on_key_view(key).await,
        }
    }

    /// Key handling when focus is on the header/tab row.
    fn on_key_tabs(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => self.prev_view(),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => self.next_view(),
            // Descend into the focused view's body.
            KeyCode::Enter | KeyCode::Down | KeyCode::Char('j') => self.focus = Focus::View,
            _ => {}
        }
    }

    /// Switches to the given view, performing any per-view entry work.
    fn set_view(&mut self, view: View) {
        self.view = view;
        // Load the log tail immediately on entering the Logs view.
        if view == View::Logs {
            self.load_logs();
        }
    }

    fn next_view(&mut self) {
        let idx = (self.view.index() + 1) % View::ALL.len();
        self.set_view(View::ALL[idx]);
    }

    fn prev_view(&mut self) {
        let idx = (self.view.index() + View::ALL.len() - 1) % View::ALL.len();
        self.set_view(View::ALL[idx]);
    }

    /// Dispatches keys to the active view when focus is on the body.
    ///
    /// `Esc` normally moves focus back up to the tab row so the user can switch views;
    /// `Tab`/`BackTab` move between views directly. The exception is when the Send view is
    /// actively editing a text field: in that case all keys (including `Esc`/`Tab`) are
    /// routed to the Send handler so they don't navigate away mid-edit.
    async fn on_key_view(&mut self, key: KeyEvent) {
        let send_editing = self.view == View::Send && self.send.editing;

        if !send_editing {
            match key.code {
                KeyCode::Esc => {
                    self.focus = Focus::Tabs;
                    return;
                }
                KeyCode::Tab => {
                    self.next_view();
                    return;
                }
                KeyCode::BackTab => {
                    self.prev_view();
                    return;
                }
                _ => {}
            }
        }

        match self.view {
            View::Accounts => views::accounts::on_key(self, key).await,
            View::Addresses => views::addresses::on_key(self, key).await,
            View::Transactions => views::transactions::on_key(self, key).await,
            View::Balances => views::balances::on_key(self, key),
            View::Send => views::send::on_key(self, key).await,
            View::Seed => views::seed::on_key(self, key).await,
            View::Logs => views::logs::on_key(self, key),
            View::Dashboard => {}
        }
    }

    /// Reads the tail of the log file into [`LogsState`].
    ///
    /// At most the last `MAX_LOG_LINES` lines are kept, to bound memory and rendering
    /// cost for a large log file.
    pub(super) fn load_logs(&mut self) {
        const MAX_LOG_LINES: usize = 2000;

        let Some(path) = self.logs.path.clone() else {
            self.logs.read_error =
                Some("Logs are written by the remote node in --rpc-url mode.".into());
            return;
        };

        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                let mut lines: Vec<String> = contents.lines().map(|l| l.to_string()).collect();
                if lines.len() > MAX_LOG_LINES {
                    lines.drain(0..lines.len() - MAX_LOG_LINES);
                }
                self.logs.lines = lines;
                self.logs.read_error = None;
            }
            Err(e) => {
                self.logs.lines.clear();
                self.logs.read_error = Some(format!("Could not read log file: {e}"));
            }
        }
    }

    /// Reveals the mnemonic phrase for the currently-selected seed account.
    pub(super) async fn reveal_seed(&mut self) {
        let Some(account) = self.data.accounts.get(self.seed.account) else {
            self.error("No account selected.");
            return;
        };
        let uuid = account.account_uuid.clone();
        match self.client.export_mnemonic(&uuid).await {
            Ok(Ok(export)) => {
                self.seed.revealed = Some(export.mnemonic);
                self.seed.revealed_seedfp = Some(export.seedfp);
                self.info("Seed phrase revealed");
            }
            Ok(Err(e)) if e.is_unlock_needed() => {
                self.error("Wallet is locked. Press 'U' to unlock first.");
            }
            Ok(Err(e)) => self.error(format!("z_exportmnemonic: {e}")),
            Err(e) => self.error(e.to_string()),
        }
    }

    // --- Prompts ----------------------------------------------------------------------

    fn open_unlock_prompt(&mut self) {
        if self.lock_state == LockState::Unencrypted {
            self.info("This wallet is not encrypted; there is no passphrase to enter.");
            return;
        }
        self.prompt = Some(Prompt {
            title: "Unlock wallet (passphrase)".into(),
            value: String::new(),
            masked: true,
            kind: PromptKind::Unlock,
        });
    }

    pub(super) fn open_new_account_prompt(&mut self) {
        self.prompt = Some(Prompt {
            title: "New account name".into(),
            value: String::new(),
            masked: false,
            kind: PromptKind::NewAccount,
        });
    }

    async fn on_key_prompt(&mut self, key: KeyEvent) {
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let prompt = self.prompt.take().expect("prompt is present");
                self.submit_prompt(prompt).await;
            }
            KeyCode::Backspace => {
                prompt.value.pop();
            }
            KeyCode::Char(c) => prompt.value.push(c),
            _ => {}
        }
    }

    async fn submit_prompt(&mut self, prompt: Prompt) {
        match prompt.kind {
            PromptKind::Unlock => {
                // Unlock for 5 minutes.
                match self.client.unlock(&prompt.value, 300).await {
                    Ok(Ok(())) => {
                        self.info("Wallet unlocked for 5 minutes");
                        // Re-check state and load data now that we have access.
                        self.refresh_lock_state().await;
                        if !self.is_gated() {
                            self.focus = Focus::View;
                            self.refresh().await;
                        }
                    }
                    // Wrong passphrase.
                    Ok(Err(e)) if e.code == -14 => {
                        self.error("Incorrect passphrase.");
                    }
                    // Wallet is not encrypted (should not happen: we guard the prompt).
                    Ok(Err(e)) if e.code == -15 => {
                        self.error("This wallet is not encrypted; nothing to unlock.");
                    }
                    Ok(Err(e)) => self.error(format!("Unlock failed: {e}")),
                    Err(e) => self.error(e.to_string()),
                }
            }
            PromptKind::NewAccount => {
                if prompt.value.trim().is_empty() {
                    self.error("Account name cannot be empty");
                    return;
                }
                match self.client.new_account(prompt.value.trim()).await {
                    Ok(Ok(_)) => {
                        self.info(format!("Created account '{}'", prompt.value.trim()));
                        self.refresh().await;
                    }
                    Ok(Err(e)) if e.is_unlock_needed() => {
                        self.error("Wallet is locked. Press 'u' to unlock first.");
                    }
                    Ok(Err(e)) => self.error(format!("z_getnewaccount: {e}")),
                    Err(e) => self.error(e.to_string()),
                }
            }
        }
    }

    // --- Lock/unlock ------------------------------------------------------------------

    async fn lock_wallet(&mut self) {
        if self.lock_state == LockState::Unencrypted {
            self.info("This wallet is not encrypted; there is nothing to lock.");
            return;
        }
        match self.client.lock().await {
            Ok(Ok(())) => {
                self.info("Wallet locked");
                self.refresh_lock_state().await;
            }
            Ok(Err(e)) => self.error(format!("walletlock: {e}")),
            Err(e) => self.error(e.to_string()),
        }
    }

    // --- Send polling -----------------------------------------------------------------

    /// Polls the pending send operation and updates its status.
    pub(super) async fn poll_send(&mut self) {
        let Some(opid) = self.send.pending_opid.clone() else {
            return;
        };
        match self.client.operation_status(&opid).await {
            Ok(Ok(mut statuses)) => {
                if let Some(status) = statuses.pop() {
                    let finished =
                        matches!(status.status.as_str(), "success" | "failed" | "cancelled");
                    if finished {
                        self.send.pending_opid = None;
                        match status.status.as_str() {
                            "success" => self.info("Send completed"),
                            "failed" => {
                                let msg = status
                                    .error
                                    .as_ref()
                                    .and_then(|e| e.message.clone())
                                    .unwrap_or_else(|| "unknown error".into());
                                self.error(format!("Send failed: {msg}"));
                            }
                            _ => self.info("Send cancelled"),
                        }
                    }
                    self.send.pending_status = Some(status);
                }
            }
            Ok(Err(e)) => self.error(format!("z_getoperationstatus: {e}")),
            Err(e) => self.error(e.to_string()),
        }
    }
}
