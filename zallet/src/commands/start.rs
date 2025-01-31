//! `start` subcommand

use abscissa_core::{config, tracing::Instrument, FrameworkError, Runnable, Shutdown};
use tokio::{pin, select};

use crate::{
    cli::StartCmd,
    components::{json_rpc, wallet::Wallet},
    config::ZalletConfig,
    error::{Error, ErrorKind},
    prelude::*,
};

impl StartCmd {
    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();

        // Open the wallet.
        let wallet = {
            let path = config
                .wallet_db
                .as_ref()
                .ok_or_else(|| ErrorKind::Init.context("wallet_db must be set (for now)"))?;
            if path.is_relative() {
                return Err(ErrorKind::Init
                    .context("wallet_db must be an absolute path (for now)")
                    .into());
            }

            info!("Opening wallet");
            Wallet::open(path, config.network(), self.lwd_server.clone()).await?
        };

        // Launch RPC server.
        let rpc_task_handle = if !config.rpc.bind.is_empty() {
            if config.rpc.bind.len() > 1 {
                return Err(ErrorKind::Init
                    .context("Only one RPC bind address is supported (for now)")
                    .into());
            }
            info!("Spawning RPC server");
            info!("Trying to open RPC endpoint at {}...", config.rpc.bind[0]);
            json_rpc::server::spawn(config.rpc.clone(), wallet.clone()).await?
        } else {
            warn!("Configure `rpc.bind` to start the RPC server");
            // Emulate a normally-operating ongoing task to simplify subsequent logic.
            tokio::spawn(std::future::pending().in_current_span())
        };

        // Start the wallet sync process.
        let wallet_sync_task_handle = wallet.spawn_sync().await?;

        info!("Spawned Zallet tasks");

        // ongoing tasks.
        pin!(rpc_task_handle);
        pin!(wallet_sync_task_handle);

        // Wait for tasks to finish.
        let res = loop {
            let mut exit_when_task_finishes = true;

            let result = select! {
                rpc_join_result = &mut rpc_task_handle => {
                    let rpc_server_result = rpc_join_result
                        .expect("unexpected panic in the RPC task");
                    info!(?rpc_server_result, "RPC task exited");
                    Ok(())
                }

                wallet_sync_join_result = &mut wallet_sync_task_handle => {
                    let wallet_sync_result = wallet_sync_join_result
                        .expect("unexpected panic in the wallet sync task");
                    info!(?wallet_sync_result, "Wallet sync task exited");
                    Ok(())
                }
            };

            // Stop Zallet if a task finished and returned an error, or if an ongoing task
            // exited.
            match result {
                Err(_) => break result,
                Ok(()) if exit_when_task_finishes => break result,
                Ok(()) => (),
            }
        };

        info!("Exiting Zallet because an ongoing task exited; asking other tasks to stop");

        // ongoing tasks
        rpc_task_handle.abort();
        wallet_sync_task_handle.abort();

        info!("All tasks have been asked to stop, waiting for remaining tasks to finish");

        res
    }
}

impl Runnable for StartCmd {
    fn run(&self) {
        match abscissa_tokio::run(&APP, self.start()) {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                eprintln!("{}", e);
                APP.shutdown(Shutdown::Forced);
            }
            Err(e) => {
                eprintln!("{}", e);
                APP.shutdown(Shutdown::Forced);
            }
        }
    }
}

impl config::Override<ZalletConfig> for StartCmd {
    fn override_config(&self, config: ZalletConfig) -> Result<ZalletConfig, FrameworkError> {
        Ok(config)
    }
}
