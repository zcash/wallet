//! JSON-RPC endpoint.
//!
//! This provides JSON-RPC methods that are (mostly) compatible with the `zcashd` wallet
//! RPCs:
//! - Some methods are exactly compatible.
//! - Some methods have the same name but slightly different semantics.
//! - Some methods from the `zcashd` wallet are unsupported.

use abscissa_core::tracing::{info, warn};
use jsonrpsee::tracing::Instrument;
use zcash_protocol::value::{COIN, Zatoshis};

use crate::{
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

use super::{TaskHandle, chain_view::ChainView, database::Database, keystore::KeyStore};

mod asyncop;
pub(crate) mod methods;
pub(crate) mod server;
pub(crate) mod utils;

// TODO: https://github.com/zcash/wallet/issues/15
fn value_from_zatoshis(value: Zatoshis) -> f64 {
    (u64::from(value) as f64) / (COIN as f64)
}

#[derive(Debug)]
pub(crate) struct JsonRpc {}

impl JsonRpc {
    pub(crate) async fn spawn(
        config: &ZalletConfig,
        db: Database,
        keystore: KeyStore,
        chain_view: ChainView,
    ) -> Result<TaskHandle, Error> {
        let rpc = config.rpc.clone();

        if !rpc.bind.is_empty() {
            if rpc.bind.len() > 1 {
                return Err(ErrorKind::Init
                    .context("Only one RPC bind address is supported (for now)")
                    .into());
            }
            info!("Spawning RPC server");
            info!("Trying to open RPC endpoint at {}...", rpc.bind[0]);
            server::spawn(rpc, db, keystore, chain_view).await
        } else {
            warn!("Configure `rpc.bind` to start the RPC server");
            // Emulate a normally-operating ongoing task to simplify subsequent logic.
            Ok(tokio::spawn(std::future::pending().in_current_span()))
        }
    }
}
