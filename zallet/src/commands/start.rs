//! `start` subcommand - example of how to write a subcommand

use abscissa_core::{config, tracing::Instrument, FrameworkError, Runnable, Shutdown};
use tokio::{pin, select};

use crate::{cli::StartCmd, components::json_rpc, config::ZalletConfig, error::Error, prelude::*};

impl StartCmd {
    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();

        // Launch RPC server.
        let rpc_task_handle = if let Some(listen_addr) = config.rpc.listen_addr {
            info!("Spawning RPC server");
            info!("Trying to open RPC endpoint at {}...", listen_addr);
            json_rpc::server::spawn(config.rpc.clone()).await?
        } else {
            warn!("Configure a listen_addr to start the RPC server");
            // Emulate a normally-operating ongoing task to simplify subsequent logic.
            tokio::spawn(std::future::pending().in_current_span())
        };

        info!("Spawned Zallet tasks");

        // ongoing tasks.
        pin!(rpc_task_handle);

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
