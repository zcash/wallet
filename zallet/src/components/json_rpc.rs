//! JSON-RPC endpoint.
//!
//! This provides JSON-RPC methods that are (mostly) compatible with the `zcashd` wallet
//! RPCs:
//! - Some methods are exactly compatible.
//! - Some methods have the same name but slightly different semantics.
//! - Some methods from the `zcashd` wallet are unsupported.

use abscissa_core::tracing::{info, warn};
use jsonrpsee::tracing::Instrument;

use crate::{
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

use super::{TaskHandle, chain::Chain, database::Database};

#[cfg(zallet_build = "wallet")]
use super::keystore::KeyStore;

#[cfg(zallet_build = "wallet")]
mod asyncop;
pub(crate) mod methods;
#[cfg(zallet_build = "wallet")]
mod payments;
pub(crate) mod server;
pub(crate) mod utils;

/// An in-memory credential for authenticating to a self-hosted ephemeral JSON-RPC server.
///
/// This is generated freshly each time the in-process TUI frontend spawns its own server,
/// and is never persisted to disk.
#[cfg(feature = "tui")]
#[derive(Clone)]
pub(crate) struct EphemeralCredential {
    user: String,
    password: secrecy::SecretString,
}

#[cfg(feature = "tui")]
impl EphemeralCredential {
    /// Generates a fresh random credential.
    pub(crate) fn generate() -> Self {
        use rand::{Rng, rngs::OsRng};

        let user_bytes: [u8; 16] = OsRng.r#gen();
        let password_bytes: [u8; 32] = OsRng.r#gen();
        Self {
            user: hex::encode(user_bytes),
            password: secrecy::SecretString::new(hex::encode(password_bytes)),
        }
    }

    /// The username portion of the credential.
    pub(crate) fn user(&self) -> &str {
        &self.user
    }

    /// The password portion of the credential.
    pub(crate) fn password(&self) -> &str {
        use secrecy::ExposeSecret;
        self.password.expose_secret()
    }
}

#[cfg(feature = "tui")]
impl std::fmt::Debug for EphemeralCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EphemeralCredential")
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[cfg(feature = "tui")]
pub(crate) use server::spawn_ephemeral;

#[derive(Debug)]
pub(crate) struct JsonRpc {}

impl JsonRpc {
    pub(crate) async fn spawn<C: Chain>(
        config: &ZalletConfig,
        db: Database,
        #[cfg(zallet_build = "wallet")] keystore: KeyStore,
        chain: C,
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
            server::spawn(
                rpc,
                db,
                #[cfg(zallet_build = "wallet")]
                keystore,
                chain,
            )
            .await
        } else {
            warn!("Configure `rpc.bind` to start the RPC server");
            // Emulate a normally-operating ongoing task to simplify subsequent logic.
            Ok(crate::spawn!(
                "No JSON-RPC",
                std::future::pending().in_current_span()
            ))
        }
    }
}
