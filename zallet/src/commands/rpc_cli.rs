//! `example-config` subcommand

use std::fmt;

use abscissa_core::{Runnable, Shutdown};
use jsonrpsee::core::{client::ClientT, params::ArrayParams};
use jsonrpsee_http_client::HttpClientBuilder;

use crate::{cli::RpcCliCmd, error::Error, prelude::*};

macro_rules! wfl {
    ($f:ident, $message_id:literal) => {
        write!($f, "{}", $crate::fl!($message_id))
    };

    ($f:ident, $message_id:literal, $($args:expr),* $(,)?) => {
        write!($f, "{}", $crate::fl!($message_id, $($args), *))
    };
}

#[allow(unused_macros)]
macro_rules! wlnfl {
    ($f:ident, $message_id:literal) => {
        writeln!($f, "{}", $crate::fl!($message_id))
    };

    ($f:ident, $message_id:literal, $($args:expr),* $(,)?) => {
        writeln!($f, "{}", $crate::fl!($message_id, $($args), *))
    };
}

impl RpcCliCmd {
    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();

        // Connect to the Zallet wallet.
        let client = match config.rpc.bind.as_slice() {
            &[] => Err(RpcCliError::WalletHasNoRpcServer),
            &[bind] => HttpClientBuilder::default()
                .build(format!("http://{bind}"))
                .map_err(|_| RpcCliError::FailedToConnect),
            addrs => addrs
                .iter()
                .find_map(|bind| {
                    HttpClientBuilder::default()
                        .build(format!("http://{bind}"))
                        .ok()
                })
                .ok_or(RpcCliError::FailedToConnect),
        }?;

        // Construct the request.
        let mut params = ArrayParams::new();
        for param in &self.params {
            let value: serde_json::Value = serde_json::from_str(param)
                .map_err(|_| RpcCliError::InvalidParameter(param.clone()))?;
            params
                .insert(value)
                .map_err(|_| RpcCliError::InvalidParameter(param.clone()))?;
        }

        // Make the request.
        let response: serde_json::Value = client
            .request(&self.command, params)
            .await
            .map_err(|e| RpcCliError::RequestFailed(e.to_string()))?;

        // Print the response.
        match response {
            serde_json::Value::String(s) => print!("{s}"),
            _ => serde_json::to_writer_pretty(std::io::stdout(), &response)
                .expect("response should be valid"),
        }

        Ok(())
    }
}

impl Runnable for RpcCliCmd {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RpcCliError {
    FailedToConnect,
    InvalidParameter(String),
    RequestFailed(String),
    WalletHasNoRpcServer,
}

impl fmt::Display for RpcCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FailedToConnect => wfl!(f, "err-rpc-cli-conn-failed"),
            Self::InvalidParameter(param) => {
                wfl!(f, "err-rpc-cli-invalid-param", parameter = param)
            }
            Self::RequestFailed(e) => {
                wfl!(f, "err-rpc-cli-request-failed", error = e)
            }
            Self::WalletHasNoRpcServer => wfl!(f, "err-rpc-cli-no-server"),
        }
    }
}

impl std::error::Error for RpcCliError {}
