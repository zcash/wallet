//! Zallet Config

use std::collections::HashMap;
use std::fmt::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use documented::{Documented, DocumentedFields};
use serde::{Deserialize, Serialize};
use zcash_protocol::consensus::NetworkType;

use crate::network::{Network, RegTestNuParam};

/// Zallet Configuration
///
/// Most fields are `Option<T>` to enable distinguishing between a user relying on a
/// default value (which may change over time), and a user explicitly configuring an
/// option with the current default value (which should be preserved). The sole exception
/// to this is `network`, which cannot change for the lifetime of the wallet.
#[derive(Clone, Debug, Deserialize, Serialize, DocumentedFields)]
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
#[derive(Clone, Debug, Default, Deserialize, Serialize, Documented, DocumentedFields)]
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
#[derive(Clone, Debug, Default, Deserialize, Serialize, Documented, DocumentedFields)]
#[serde(deny_unknown_fields)]
pub struct IndexerSection {
    /// IP address and port of the JSON-RPC interface for the full node / validator being
    /// used as a data source.
    ///
    /// If unset, connects on localhost to the standard JSON-RPC port for mainnet or
    /// testnet (as appropriate).
    pub validator_address: Option<String>,

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
#[derive(Clone, Debug, Default, Deserialize, Serialize, Documented, DocumentedFields)]
#[serde(deny_unknown_fields)]
pub struct KeyStoreSection {
    /// Path to the age identity file that encrypts key material.
    // TODO: Change this to `PathBuf` once `age::IdentityFile::from_file` supports it.
    pub identity: String,
}

/// Limits configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Documented, DocumentedFields)]
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
#[derive(Clone, Debug, Default, Deserialize, Serialize, Documented, DocumentedFields)]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

impl ZalletConfig {
    /// Generates an example config file, with all default values included as comments.
    pub fn generate_example() -> String {
        // This is the one bit of duplication we can't yet avoid. It could be replaced
        // with a proc macro, but for now we just need to remember to update this as we
        // make changes to the config structure.
        let conf = ZalletConfig::default();
        let field_defaults = [
            top("broadcast", conf.broadcast()),
            top("export_dir", &conf.export_dir),
            top("network", crate::network::kind::Serializable(conf.network)),
            top("notify", &conf.notify),
            top("regtest_nuparams", &conf.regtest_nuparams),
            top("require_backup", conf.require_backup()),
            top("wallet_db", &conf.wallet_db),
            builder(
                "spend_zeroconf_change",
                conf.builder.spend_zeroconf_change(),
            ),
            builder("tx_expiry_delta", conf.builder.tx_expiry_delta()),
            indexer("validator_address", conf.indexer.validator_address),
            indexer("validator_cookie_auth", conf.indexer.validator_cookie_auth),
            indexer("validator_cookie_path", &conf.indexer.validator_cookie_path),
            indexer("validator_user", &conf.indexer.validator_user),
            indexer("validator_password", &conf.indexer.validator_password),
            indexer("db_path", &conf.indexer.db_path),
            keystore("identity", &conf.keystore.identity),
            limits("orchard_actions", conf.limits.orchard_actions()),
            rpc("bind", &conf.rpc.bind),
            rpc("timeout", conf.rpc.timeout().as_secs()),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        // The glue that makes the above easy to maintain:
        const BUILDER: &str = "builder";
        const INDEXER: &str = "indexer";
        const KEYSTORE: &str = "keystore";
        const LIMITS: &str = "limits";
        const RPC: &str = "rpc";
        fn top<T: Serialize>(
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            field("", f, d)
        }
        fn builder<T: Serialize>(
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            field(BUILDER, f, d)
        }
        fn indexer<T: Serialize>(
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            field(INDEXER, f, d)
        }
        fn keystore<T: Serialize>(
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            field(KEYSTORE, f, d)
        }
        fn limits<T: Serialize>(
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            field(LIMITS, f, d)
        }
        fn rpc<T: Serialize>(
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            field(RPC, f, d)
        }
        fn field<T: Serialize>(
            s: &'static str,
            f: &'static str,
            d: T,
        ) -> ((&'static str, &'static str), Option<toml::Value>) {
            (
                (s, f),
                match toml::Value::try_from(d) {
                    Ok(v) => Some(v),
                    Err(e) if e.to_string() == "unsupported None value" => None,
                    Err(_) => unreachable!(),
                },
            )
        }

        let top_def = |field_name| {
            field_defaults
                .get(&("", field_name))
                .expect("need to update field_defaults with changes to ZalletConfig")
                .as_ref()
        };

        let sec_def = |section_name, field_name| {
            field_defaults
                .get(&(section_name, field_name))
                .expect("need to update field_defaults with changes to ZalletConfig")
                .as_ref()
        };

        let mut config = r"# Default configuration for Zallet.
#
# This file is generated as an example using Zallet's current defaults. It can
# be used as a skeleton for custom configs.
#
# Fields that are required to be set are uncommented, and set to an example
# value. Every other field is commented out, and set to the current default
# value that Zallet will use for it (or `UNSET` if the field has no default).
#
# Leaving a field commented out means that Zallet will always use the latest
# default value, even if it changes in future. Uncommenting a field but keeping
# it set to the current default value means that Zallet will treat it as a
# user-configured value going forward.

"
        .to_owned();

        fn write_section<'a, T: Documented + DocumentedFields>(
            config: &mut String,
            section_name: &'static str,
            sec_def: impl Fn(&'static str, &'static str) -> Option<&'a toml::Value>,
        ) {
            writeln!(config).unwrap();
            for line in T::DOCS.lines() {
                writeln!(config, "# {line}").unwrap();
            }
            writeln!(config, "[{section_name}]").unwrap();
            writeln!(config).unwrap();

            for field_name in T::FIELD_NAMES {
                write_field::<T>(config, field_name, false, sec_def(section_name, field_name));
            }
        }

        fn write_field<T: DocumentedFields>(
            config: &mut String,
            field_name: &str,
            required: bool,
            field_default: Option<&toml::Value>,
        ) {
            let field_doc = T::get_field_docs(field_name).expect("present");
            for line in field_doc.lines() {
                if line.is_empty() {
                    writeln!(config, "#").unwrap();
                } else {
                    writeln!(config, "# {line}").unwrap();
                }
            }

            write!(
                config,
                "{}{} = ",
                if required { "" } else { "#" },
                field_name
            )
            .unwrap();
            match field_default {
                Some(present) => {
                    Serialize::serialize(&present, toml::ser::ValueSerializer::new(config)).unwrap()
                }
                None => write!(config, "UNSET").unwrap(),
            }

            writeln!(config).unwrap();
            writeln!(config).unwrap();
        }

        for field_name in Self::FIELD_NAMES {
            match *field_name {
                BUILDER => write_section::<BuilderSection>(&mut config, field_name, sec_def),
                INDEXER => write_section::<IndexerSection>(&mut config, field_name, sec_def),
                KEYSTORE => write_section::<KeyStoreSection>(&mut config, field_name, sec_def),
                LIMITS => write_section::<LimitsSection>(&mut config, field_name, sec_def),
                RPC => write_section::<RpcSection>(&mut config, field_name, sec_def),
                _ => write_field::<Self>(
                    &mut config,
                    field_name,
                    *field_name == "network",
                    top_def(field_name),
                ),
            }
        }

        config
    }
}
