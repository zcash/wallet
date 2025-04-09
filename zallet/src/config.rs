//! Zallet Config

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zcash_protocol::consensus::NetworkType;

use crate::network::{Network, RegTestNuParam};

/// Zallet Configuration
///
/// Most fields are `Option<T>` to enable distinguishing between a user relying on a
/// default value (which may change over time), and a user explicitly configuring an
/// option with the current default value (which should be preserved). The sole exception
/// to this is `network`, which cannot change for the lifetime of the wallet.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZalletConfig {
    /// Whether the wallet should broadcast transactions.
    pub broadcast: Option<bool>,

    /// Directory to be used when exporting data.
    pub export_dir: Option<String>,

    /// Network type.
    #[serde(with = "crate::network::kind")]
    pub network: NetworkType,

    /// Execute command when a wallet transaction changes.
    ///
    /// `%s` in the command is replaced by TxID.
    pub notify: Option<String>,

    /// The parameters for regtest mode.
    ///
    /// Ignored if `network` is not `NetworkType::Regtest`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regtest_nuparams: Vec<RegTestNuParam>,

    /// By default, the wallet will not allow generation of new spending keys & addresses
    /// from the mnemonic seed until the backup of that seed has been confirmed with the
    /// `zcashd-wallet-tool` utility. A user may start zallet with `--walletrequirebackup=false`
    /// to allow generation of spending keys even if the backup has not yet been confirmed.
    pub require_backup: Option<bool>,

    /// Path to the wallet database file.
    ///
    /// TODO: If we decide to support a data directory, allow this to have a relative path
    /// within it as well as a default name.
    pub wallet_db: Option<PathBuf>,

    /// Settings that affect transactions created by Zallet.
    pub builder: BuilderSection,

    /// Settings for the Zaino chain indexer.
    pub indexer: IndexerSection,

    /// Settings for the key store.
    pub keystore: KeyStoreSection,

    /// Configurable limits on wallet operation (to prevent e.g. memory exhaustion).
    pub limits: LimitsSection,

    /// Settings for the JSON-RPC interface.
    pub rpc: RpcSection,
}

impl Default for ZalletConfig {
    fn default() -> Self {
        Self {
            broadcast: None,
            export_dir: None,
            network: NetworkType::Main,
            notify: None,
            regtest_nuparams: vec![],
            require_backup: None,
            wallet_db: None,
            builder: Default::default(),
            indexer: Default::default(),
            keystore: Default::default(),
            limits: Default::default(),
            rpc: Default::default(),
        }
    }
}

impl ZalletConfig {
    /// Whether the wallet should broadcast transactions.
    ///
    /// Default is `true`.
    pub fn broadcast(&self) -> bool {
        self.broadcast.unwrap_or(true)
    }

    /// Returns the network parameters for this wallet.
    pub fn network(&self) -> Network {
        Network::from_type(self.network, &self.regtest_nuparams)
    }

    /// Whether to require a confirmed wallet backup.
    ///
    /// By default, the wallet will not allow generation of new spending keys & addresses
    /// from the mnemonic seed until the backup of that seed has been confirmed with the
    /// `zcashd-wallet-tool` utility. A user may start zallet with `--walletrequirebackup=false`
    /// to allow generation of spending keys even if the backup has not yet been confirmed.
    pub fn require_backup(&self) -> bool {
        self.require_backup.unwrap_or(true)
    }
}

/// Transaction builder configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderSection {
    /// Whether to spend unconfirmed transparent change when sending transactions.
    ///
    /// Does not affect unconfirmed shielded change, which cannot be spent.
    pub spend_zeroconf_change: Option<bool>,

    /// The number of blocks after which a transaction created by Zallet that has not been
    /// mined will become invalid.
    ///
    /// - Minimum: `TX_EXPIRING_SOON_THRESHOLD + 1`
    pub tx_expiry_delta: Option<u16>,
}

impl BuilderSection {
    /// Whether to spend unconfirmed transparent change when sending transactions.
    ///
    /// Default is `true`.
    ///
    /// Does not affect unconfirmed shielded change, which cannot be spent.
    pub fn spend_zeroconf_change(&self) -> bool {
        self.spend_zeroconf_change.unwrap_or(true)
    }

    /// The number of blocks after which a transaction created by Zallet that has not been
    /// mined will become invalid.
    ///
    /// - Minimum: `TX_EXPIRING_SOON_THRESHOLD + 1`
    /// - Default: 40
    pub fn tx_expiry_delta(&self) -> u16 {
        self.tx_expiry_delta.unwrap_or(40)
    }
}

/// Indexer configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexerSection {
    /// Full node / validator listen port.
    pub validator_listen_address: Option<SocketAddr>,

    /// Enable validator RPC cookie authentication.
    pub validator_cookie_auth: Option<bool>,

    /// Path to the validator cookie file.
    pub validator_cookie_path: Option<String>,

    /// Full node / validator Username.
    pub validator_user: Option<String>,

    /// Full node / validator Password.
    pub validator_password: Option<String>,

    /// Block Cache database file path.
    ///
    /// This is Zaino's Compact Block Cache db if using the FetchService or Zebra's RocksDB if using the StateService.
    pub db_path: Option<PathBuf>,
}

/// Key store configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyStoreSection {
    /// Path to the age identity file that encrypts key material.
    // TODO: Change this to `PathBuf` once `age::IdentityFile::from_file` supports it.
    pub identity: String,
}

/// Limits configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsSection {
    /// The maximum number of Orchard actions permitted in a constructed transaction.
    pub orchard_actions: Option<u16>,
}

impl LimitsSection {
    /// The maximum number of Orchard actions permitted in a constructed transaction.
    ///
    /// Default is 50.
    pub fn orchard_actions(&self) -> u16 {
        self.orchard_actions.unwrap_or(50)
    }
}

/// RPC configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RpcSection {
    /// Addresses to listen for JSON-RPC connections.
    ///
    /// Note: The RPC server is disabled by default. To enable the RPC server, set a
    /// listen address in the config:
    /// ```toml
    /// [rpc]
    /// bind = ["127.0.0.1:28232"]
    /// ```
    ///
    /// # Security
    ///
    /// If you bind Zallet's RPC port to a public IP address, anyone on the internet can
    /// view your transactions and spend your funds.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bind: Vec<SocketAddr>,

    /// Timeout (in seconds) during HTTP requests.
    pub timeout: Option<u64>,
}

impl RpcSection {
    /// Timeout during HTTP requests.
    ///
    /// Default is 30 seconds.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(30))
    }
}
