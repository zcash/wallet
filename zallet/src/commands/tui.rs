//! `tui` subcommand: an interactive terminal frontend for the wallet.
//!
//! The TUI is fundamentally a JSON-RPC client. It supports two modes:
//!
//! - **Self-hosted (default):** boots the full wallet backend in-process (database,
//!   keystore, chain, and sync tasks), spawns a JSON-RPC server bound to an ephemeral
//!   loopback port with an in-memory credential, and then connects to that server as an
//!   HTTP client. This holds the data directory lock for the duration of the session.
//!
//! - **Remote (`--rpc-url`):** connects to an already-running `zallet start` instance over
//!   HTTP, using the `[[rpc.auth]]` credentials from the configuration. No backend is
//!   booted and the data directory lock is not taken.
//!
//! In both modes the UI drives the wallet exclusively through the JSON-RPC interface, so
//! there is a single tested code path regardless of where the server lives.

use std::time::Duration;

use abscissa_core::Runnable;
use tokio::{pin, select};

use crate::{
    cli::TuiCmd,
    commands::AsyncRunnable,
    components::{chain::ZainoChain, database::Database, keystore::KeyStore, sync::WalletSync},
    error::{Error, ErrorKind},
    prelude::*,
};

mod app;
mod client;
mod event;
mod qr;
mod terminal;
mod ui;
mod views;

const DEFAULT_HTTP_CLIENT_TIMEOUT: u64 = 900;

impl AsyncRunnable for TuiCmd {
    async fn run(&self) -> Result<(), Error> {
        let timeout = Duration::from_secs(match self.timeout {
            Some(0) => u64::MAX,
            Some(timeout) => timeout,
            None => DEFAULT_HTTP_CLIENT_TIMEOUT,
        });

        if let Some(rpc_url) = &self.rpc_url {
            self.run_remote(rpc_url, timeout).await
        } else {
            self.run_self_hosted(timeout).await
        }
    }
}

impl TuiCmd {
    /// Connects to a remote `zallet start` instance and runs the UI.
    async fn run_remote(&self, rpc_url: &str, timeout: Duration) -> Result<(), Error> {
        let config = APP.config();

        let client = client::WalletClient::connect_remote(rpc_url, &config.rpc.auth, timeout)
            .map_err(|e| ErrorKind::Generic.context(e))?;

        // In remote mode logs are written by the remote `zallet start`, not locally.
        run_ui(client, None).await
    }

    /// Boots the wallet backend in-process and runs the UI against a self-hosted ephemeral
    /// JSON-RPC server.
    async fn run_self_hosted(&self, timeout: Duration) -> Result<(), Error> {
        let config = APP.config();
        let _lock = config.lock_datadir()?;

        let db = Database::open(&config).await?;
        let keystore = KeyStore::new(&config, db.clone())?;

        // Start monitoring the chain.
        let (chain, chain_indexer_task_handle) = ZainoChain::new(&config).await?;

        // Launch an ephemeral JSON-RPC server bound to loopback.
        let (rpc_task_handle, rpc_addr, credential) = crate::components::json_rpc::spawn_ephemeral(
            timeout,
            db.clone(),
            keystore,
            chain.clone(),
        )
        .await?;

        // Start the wallet sync process so balances and history stay live.
        let (
            wallet_sync_steady_state_task_handle,
            wallet_sync_recover_history_task_handle,
            wallet_sync_batch_decryptor_task_handle,
            wallet_sync_data_requests_task_handle,
        ) = WalletSync::spawn(&config, db, chain).await?;

        info!("Spawned Zallet TUI backend tasks");

        // Connect the UI client to our own server.
        let client = client::WalletClient::connect_local(rpc_addr, &credential, timeout)
            .map_err(|e| ErrorKind::Generic.context(e))?;

        // The self-hosted backend writes its logs to `<datadir>/tui.log` (see
        // `EntryPoint::tui_log_path`); surface that path to the Logs view.
        let log_path = crate::commands::resolve_datadir_path(
            config.datadir(),
            std::path::Path::new("tui.log"),
        );

        pin!(chain_indexer_task_handle);
        pin!(rpc_task_handle);
        pin!(wallet_sync_steady_state_task_handle);
        pin!(wallet_sync_recover_history_task_handle);
        pin!(wallet_sync_batch_decryptor_task_handle);
        pin!(wallet_sync_data_requests_task_handle);

        // Run the UI, racing it against the backend tasks. If any backend task exits
        // (which should only happen on failure), tear down the UI and surface the result.
        let res = select! {
            ui_result = run_ui(client, Some(log_path)) => ui_result,

            join = &mut chain_indexer_task_handle => {
                let r = join.expect("unexpected panic in the chain indexer task");
                info!(?r, "Chain indexer task exited");
                r
            }
            join = &mut rpc_task_handle => {
                let r = join.expect("unexpected panic in the RPC task");
                info!(?r, "RPC task exited");
                r
            }
            join = &mut wallet_sync_steady_state_task_handle => {
                let r = join.expect("unexpected panic in the wallet steady-state sync task");
                info!(?r, "Wallet steady-state sync task exited");
                r
            }
            join = &mut wallet_sync_recover_history_task_handle => {
                let r = join.expect("unexpected panic in the wallet recover-history sync task");
                info!(?r, "Wallet recover-history sync task exited");
                r
            }
            join = &mut wallet_sync_batch_decryptor_task_handle => {
                let r = join.expect("unexpected panic in the wallet batch decryptor task");
                info!(?r, "Wallet batch decryptor task exited");
                r
            }
            join = &mut wallet_sync_data_requests_task_handle => {
                let r = join.expect("unexpected panic in the wallet data-requests sync task");
                info!(?r, "Wallet data-requests sync task exited");
                r
            }
        };

        info!("Shutting down Zallet TUI backend tasks");

        chain_indexer_task_handle.abort();
        rpc_task_handle.abort();
        wallet_sync_steady_state_task_handle.abort();
        wallet_sync_recover_history_task_handle.abort();
        wallet_sync_batch_decryptor_task_handle.abort();
        wallet_sync_data_requests_task_handle.abort();

        res
    }
}

/// Runs the terminal UI event loop against the given wallet client.
///
/// The terminal is placed into raw mode and the alternate screen on entry, and restored on
/// exit (including on panic, via [`terminal::TerminalGuard`]).
async fn run_ui(
    client: client::WalletClient,
    log_path: Option<std::path::PathBuf>,
) -> Result<(), Error> {
    let mut guard = terminal::TerminalGuard::enter().map_err(|e| ErrorKind::Generic.context(e))?;

    let mut app = app::App::new(client, log_path);
    let result = app.run(guard.terminal_mut()).await;

    // Restore the terminal before returning, so any error is printed cleanly.
    guard.restore();

    result
}

impl Runnable for TuiCmd {
    fn run(&self) {
        self.run_on_runtime();
        info!("Exited Zallet TUI");
    }
}
