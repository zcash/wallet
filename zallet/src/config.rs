//! Zallet Config

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Zallet Configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZalletConfig {
    pub rpc: RpcSection,
}

/// Default configuration settings.
impl Default for ZalletConfig {
    fn default() -> Self {
        Self {
            rpc: RpcSection::default(),
        }
    }
}

/// RPC configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RpcSection {
    /// IP address and port for the RPC server.
    ///
    /// Note: The RPC server is disabled by default. To enable the RPC server, set a
    /// listen address in the config:
    /// ```toml
    /// [rpc]
    /// listen_addr = '127.0.0.1:28232'
    /// ```
    ///
    /// # Security
    ///
    /// If you bind Zallet's RPC port to a public IP address, anyone on the internet can
    /// view your transactions and spend your funds.
    pub listen_addr: Option<SocketAddr>,
}

impl Default for RpcSection {
    fn default() -> Self {
        Self {
            // Disable RPCs by default.
            listen_addr: None,
        }
    }
}
