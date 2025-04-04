//! `start` subcommand

use abscissa_core::{FrameworkError, Runnable, Shutdown, config};
use tokio::{pin, select};

use crate::{
    cli::StartCmd,
    components::{database::Database, json_rpc::JsonRpc, keystore::KeyStore, sync::WalletSync},
    config::ZalletConfig,
    error::Error,
    prelude::*,
};

impl StartCmd {
    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();

        let db = Database::open(&config).await?;
        let keystore = KeyStore::new(&config, db.clone())?;

        // Launch RPC server.
        let rpc_task_handle = JsonRpc::spawn(&config, db.clone(), keystore).await?;

        // Start the wallet sync process.
        let wallet_sync_task_handle = WalletSync::spawn(&config, db, self.lwd_server.clone()).await;

        info!("Spawned Zallet tasks");

        // ongoing tasks.
        pin!(rpc_task_handle);
        pin!(wallet_sync_task_handle);

        // Wait for tasks to finish.
        let res = loop {
            let exit_when_task_finishes = true;

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
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
            Err(e) => {
                eprintln!("{}", e);
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
        }
    }
}

impl config::Override<ZalletConfig> for StartCmd {
    fn override_config(&self, config: ZalletConfig) -> Result<ZalletConfig, FrameworkError> {
        Ok(config)
    }
}
