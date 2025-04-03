//! JSON-RPC endpoint.
//!
//! This provides JSON-RPC methods that are (mostly) compatible with the `zcashd` wallet
//! RPCs:
//! - Some methods are exactly compatible.
//! - Some methods have the same name but slightly different semantics.
//! - Some methods from the `zcashd` wallet are unsupported.

use std::fmt;

use abscissa_core::{
    component::Injectable,
    tracing::{info, warn},
    Component, FrameworkError, FrameworkErrorKind,
};
use abscissa_tokio::TokioComponent;
use tokio::task::JoinHandle;
use zcash_protocol::value::{Zatoshis, COIN};

use crate::{
    application::ZalletApp,
    config::{RpcSection, ZalletConfig},
    error::{Error, ErrorKind},
};

use super::{database::Database, keystore::KeyStore};

pub(crate) mod methods;
pub(crate) mod server;
mod utils;

// TODO: https://github.com/zcash/wallet/issues/15
fn value_from_zatoshis(value: Zatoshis) -> f64 {
    (u64::from(value) as f64) / (COIN as f64)
}

#[derive(Default, Injectable)]
#[component(inject = "init_db(zallet::components::database::Database)")]
#[component(inject = "init_keystore(zallet::components::keystore::KeyStore)")]
#[component(inject = "init_tokio(abscissa_tokio::TokioComponent)")]
pub(crate) struct JsonRpc {
    rpc: Option<RpcSection>,
    db: Option<Database>,
    keystore: Option<KeyStore>,
    pub(crate) rpc_task: Option<JoinHandle<Result<(), Error>>>,
}

impl fmt::Debug for JsonRpc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonRpc").finish_non_exhaustive()
    }
}

impl Component<ZalletApp> for JsonRpc {
    fn after_config(&mut self, config: &ZalletConfig) -> Result<(), FrameworkError> {
        if config.rpc.bind.len() > 1 {
            return Err(FrameworkErrorKind::ConfigError
                .context(
                    ErrorKind::Init.context("Only one RPC bind address is supported (for now)"),
                )
                .into());
        }
        self.rpc = Some(config.rpc.clone());
        Ok(())
    }
}

impl JsonRpc {
    /// Called automatically after `Database` is initialized
    pub fn init_db(&mut self, db: &Database) -> Result<(), FrameworkError> {
        self.db = Some(db.clone());
        Ok(())
    }

    /// Called automatically after `KeyStore` is initialized
    pub fn init_keystore(&mut self, keystore: &KeyStore) -> Result<(), FrameworkError> {
        self.keystore = Some(keystore.clone());
        Ok(())
    }

    /// Called automatically after `TokioComponent` is initialized
    pub fn init_tokio(&mut self, tokio_cmp: &TokioComponent) -> Result<(), FrameworkError> {
        let rpc = self.rpc.clone().expect("configured");
        let db = self.db.clone().expect("Database initialized");
        let keystore = self.keystore.clone().expect("KeyStore initialized");

        let runtime = tokio_cmp.runtime()?;

        let task = runtime.spawn(async move {
            if !rpc.bind.is_empty() {
                if rpc.bind.len() > 1 {
                    return Err(ErrorKind::Init
                        .context("Only one RPC bind address is supported (for now)")
                        .into());
                }
                info!("Spawning RPC server");
                info!("Trying to open RPC endpoint at {}...", rpc.bind[0]);
                server::start(rpc, db, keystore).await
            } else {
                warn!("Configure `rpc.bind` to start the RPC server");
                Ok(())
            }
        });

        self.rpc_task = Some(task);

        Ok(())
    }
}
