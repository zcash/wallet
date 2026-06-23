use std::path::PathBuf;

use clap::{Parser, builder::Styles};

#[cfg(feature = "query")]
use clap::Subcommand;

#[cfg(zallet_build = "wallet")]
use uuid::Uuid;

#[cfg(outside_buildscript)]
use abscissa_core::{Command, Runnable};

use crate::fl;

#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
#[command(author, about, version)]
#[command(help_template = format!("\
{{before-help}}{{about-with-newline}}
{}{}:{} {{usage}}

{{all-args}}{{after-help}}\
    ",
    Styles::default().get_usage().render(),
    fl!("usage-header"),
    Styles::default().get_usage().render_reset()))]
#[command(next_help_heading = fl!("flags-header"))]
pub struct EntryPoint {
    #[command(subcommand)]
    pub(crate) cmd: ZalletCmd,

    /// Enable verbose logging
    #[arg(short, long)]
    pub(crate) verbose: bool,

    /// Specify the data directory for the Zallet wallet.
    ///
    /// This must be an absolute path.
    #[arg(short, long)]
    pub(crate) datadir: Option<PathBuf>,

    /// Use the specified configuration file.
    ///
    /// Relative paths will be prefixed by the datadir.
    #[arg(short, long)]
    pub(crate) config: Option<PathBuf>,
}

#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command, Runnable))]
pub(crate) enum ZalletCmd {
    /// The `start` subcommand
    Start(StartCmd),

    /// Generate an example `zallet.toml` config.
    ExampleConfig(ExampleConfigCmd),

    /// Generate a `zallet.toml` config from an existing `zcash.conf` file.
    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    MigrateZcashConf(MigrateZcashConfCmd),

    /// Add the keys and transactions of a zcashd wallet.dat file to the wallet database.
    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    MigrateZcashdWallet(MigrateZcashdWalletCmd),

    /// Generate the wallet's age encryption identity.
    #[cfg(zallet_build = "wallet")]
    GenerateEncryptionIdentity(GenerateEncryptionIdentityCmd),

    /// Initialize wallet encryption.
    #[cfg(zallet_build = "wallet")]
    InitWalletEncryption(InitWalletEncryptionCmd),

    /// Generate a BIP 39 mnemonic phrase and store it in the wallet.
    #[cfg(zallet_build = "wallet")]
    GenerateMnemonic(GenerateMnemonicCmd),

    /// Import a BIP 39 mnemonic phrase into the wallet.
    #[cfg(zallet_build = "wallet")]
    ImportMnemonic(ImportMnemonicCmd),

    /// Export an encrypted BIP 39 mnemonic phrase from the wallet.
    #[cfg(zallet_build = "wallet")]
    ExportMnemonic(ExportMnemonicCmd),

    /// Adds a user authorization for the JSON-RPC interface.
    AddRpcUser(AddRpcUserCmd),

    /// Communicate with a Zallet wallet's JSON-RPC interface.
    #[cfg(feature = "rpc-cli")]
    Rpc(RpcCliCmd),

    /// Run the interactive terminal UI for the wallet.
    #[cfg(all(feature = "tui", zallet_build = "wallet"))]
    Tui(TuiCmd),

    /// Query a Zallet wallet's JSON-RPC interface using typed CLI arguments.
    #[cfg(feature = "query")]
    Query(QueryCmd),

    /// Commands for repairing broken wallet states.
    #[command(subcommand)]
    Repair(RepairCmd),

    /// Hidden regtest-only commands.
    #[command(subcommand, hide = true)]
    Regtest(RegtestCmd),
}

/// `start` subcommand
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct StartCmd {}

/// `example-config` subcommand
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct ExampleConfigCmd {
    /// Where to write the Zallet config file.
    ///
    /// - By default, the default Zallet config file path is used.
    /// - The value `-` will write the config to stdout.
    #[arg(short, long)]
    pub(crate) output: Option<String>,

    /// Force an existing Zallet config file to be overwritten.
    #[arg(short, long)]
    pub(crate) force: bool,

    /// Temporary flag ensuring any alpha users are aware the config is not stable.
    #[arg(long)]
    pub(crate) this_is_alpha_code_and_you_will_need_to_recreate_the_example_later: bool,
}

/// `migrate-zcash-conf` subcommand
#[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct MigrateZcashConfCmd {
    /// Specify `zcashd` configuration file.
    ///
    /// Relative paths will be prefixed by `zcashd_datadir` location.
    #[arg(long, default_value = "zcash.conf")]
    pub(crate) conf: PathBuf,

    /// Specify `zcashd` data directory (this path cannot use '~').
    #[arg(long)]
    pub(crate) zcashd_datadir: Option<PathBuf>,

    /// Allow a migration when warnings are present.
    #[arg(long)]
    pub(crate) allow_warnings: bool,

    /// Where to write the Zallet config file.
    ///
    /// - By default, the default Zallet config file path is used.
    /// - The value `-` will write the config to stdout.
    #[arg(short, long)]
    pub(crate) output: Option<String>,

    /// Force an existing Zallet config file to be overwritten.
    #[arg(short, long)]
    pub(crate) force: bool,

    /// Temporary flag ensuring any alpha users are aware the migration is not stable.
    #[arg(long)]
    pub(crate) this_is_alpha_code_and_you_will_need_to_redo_the_migration_later: bool,
}

/// `migrate-zcashd-wallet` subcommand
#[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct MigrateZcashdWalletCmd {
    /// Specify location of the `zcashd` `wallet.dat` file.
    ///
    /// Relative paths will be prefixed by `zcashd_datadir` location.
    #[arg(long, default_value = "wallet.dat")]
    pub(crate) path: PathBuf,

    /// Specify `zcashd` data directory (this path cannot use '~').
    #[arg(long)]
    pub(crate) zcashd_datadir: Option<PathBuf>,

    /// Buffer wallet transactions in-memory in the process of performing the wallet restore. For
    /// very active wallets, this might exceed the available memory on your machine, so enable this
    /// with caution.
    #[arg(long)]
    pub(crate) buffer_wallet_transactions: bool,

    /// Allow import of wallet data from multiple `zcashd` `wallet.dat` files. Each imported wallet
    /// will create a distinct set of accounts in `zallet`. Attempts to import wallet data
    /// corresponding to an already-imported wallet will result in an error.
    #[arg(long)]
    pub(crate) allow_multiple_wallet_imports: bool,

    /// Specify the path to the zcashd installation directory.
    ///
    /// This is required for locating the `db_dump` command used to extract data from the
    /// `wallet.dat` file. Wallet migration without a local `zcashd` installation is not yet
    /// supported.
    #[arg(long)]
    pub(crate) zcashd_install_dir: Option<PathBuf>,

    /// Allow a migration when warnings are present. If set to `false`, any warning will be treated
    /// as an error and cause the migration to abort. Setting this to `true` will allow the import
    /// of partially-corrupted wallets, or wallets that contain transaction data from consensus
    /// forks of the Zcash chain (only transaction data corresponding to known consensus rules will
    /// be imported.)
    #[arg(long)]
    pub(crate) allow_warnings: bool,

    /// Skip chain scanning during migration. Keys, accounts, and transaction data are
    /// still imported, but block heights and tree state are not resolved from the chain.
    /// Useful when the corresponding chain data is not available.
    #[arg(long)]
    pub(crate) no_scan: bool,

    /// Temporary flag ensuring any alpha users are aware the migration is not stable.
    #[arg(long)]
    pub(crate) this_is_alpha_code_and_you_will_need_to_redo_the_migration_later: bool,
}

/// `generate-encryption-identity` subcommand
#[cfg(zallet_build = "wallet")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct GenerateEncryptionIdentityCmd {
    /// Where to write the age encryption identity file.
    ///
    /// - By default, the configured `keystore.encryption_identity` path is used.
    /// - The value `-` will write the identity to stdout.
    #[arg(short, long)]
    pub(crate) output: Option<String>,

    /// Encrypt the identity with a passphrase (ASCII-armored).
    ///
    /// In non-interactive contexts, the passphrase is read from the
    /// `ZALLET_IDENTITY_PASSPHRASE` environment variable; otherwise
    /// you will be prompted for it.
    #[arg(short, long)]
    pub(crate) passphrase: bool,
}

/// `init-wallet-encryption` subcommand
#[cfg(zallet_build = "wallet")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct InitWalletEncryptionCmd {}

/// `generate-mnemonic` subcommand
#[cfg(zallet_build = "wallet")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct GenerateMnemonicCmd {}

/// `import-mnemonic` subcommand
#[cfg(zallet_build = "wallet")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct ImportMnemonicCmd {}

/// `export-mnemonic` subcommand
#[cfg(zallet_build = "wallet")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct ExportMnemonicCmd {
    /// Output in a PEM encoded format.
    #[arg(short, long)]
    pub(crate) armor: bool,

    /// The UUID of the account from which to export.
    pub(crate) account_uuid: Uuid,
}

/// `add-rpc-user` subcommand
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct AddRpcUserCmd {
    /// The username to add.
    pub(crate) username: String,
}

/// `rpc` subcommand
#[cfg(feature = "rpc-cli")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct RpcCliCmd {
    /// Client timeout in seconds during HTTP requests, or 0 for no timeout.
    ///
    /// Default is 900 seconds.
    ///
    /// The server timeout is configured with the `rpc.timeout` option in the
    /// configuration file.
    #[arg(long)]
    pub(crate) timeout: Option<u64>,

    /// The JSON-RPC command to send to Zallet.
    ///
    /// Use `zallet rpc help` to get a list of RPC endpoints.
    pub(crate) command: String,

    /// Any parameters for the command.
    pub(crate) params: Vec<String>,
}

/// `tui` subcommand
#[cfg(all(feature = "tui", zallet_build = "wallet"))]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct TuiCmd {
    /// Connect to an already-running Zallet JSON-RPC server at this URL instead of
    /// starting one.
    ///
    /// The value is the URL of the remote server (e.g. `http://127.0.0.1:28232`); a bare
    /// `host:port` is also accepted and assumed to be `http`. Authentication is taken from
    /// the `[[rpc.auth]]` entries in the configuration file, unless credentials are
    /// embedded in the URL.
    ///
    /// When omitted, the TUI starts its own wallet backend and JSON-RPC server in-process
    /// (bound to an ephemeral loopback port with an in-memory credential), and holds the
    /// data directory lock for the duration of the session.
    #[arg(long, value_name = "URL")]
    pub(crate) rpc_url: Option<String>,

    /// Client timeout in seconds during HTTP requests, or 0 for no timeout.
    ///
    /// Default is 900 seconds.
    #[arg(long)]
    pub(crate) timeout: Option<u64>,
}

/// `query` subcommand
#[cfg(feature = "query")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
// The `help` JSON-RPC method is exposed as a `help` subcommand, which would
// otherwise collide with clap's automatically-generated `help` subcommand.
#[command(disable_help_subcommand = true)]
pub(crate) struct QueryCmd {
    /// Connect to an already-running Zallet JSON-RPC server at this URL instead of
    /// starting one.
    ///
    /// The value is the URL of the remote server (e.g. `http://127.0.0.1:28232`); a bare
    /// `host:port` is also accepted and assumed to be `http`. Authentication is taken from
    /// the `[[rpc.auth]]` entries in the configuration file, unless credentials are
    /// embedded in the URL.
    ///
    /// When omitted, an in-process wallet backend and JSON-RPC server are started (on the
    /// configured loopback `rpc.bind` address, authenticated via the RPC cookie in the data
    /// directory), the request is made over loopback, and everything is shut down once the
    /// request completes. The data directory lock is held for the duration.
    #[arg(long, value_name = "URL", global = true)]
    pub(crate) rpc_url: Option<String>,

    /// Client timeout in seconds during HTTP requests, or 0 for no timeout.
    ///
    /// Default is 900 seconds.
    #[arg(long, global = true)]
    pub(crate) timeout: Option<u64>,

    /// Read the wallet passphrase from the first line of standard input, rather than
    /// prompting for it interactively.
    ///
    /// This is only consulted if the method being called requires the wallet to be
    /// unlocked (e.g. `z_sendmany`). The `ZALLET_PASSPHRASE` environment variable, if set,
    /// takes precedence over both this flag and the interactive prompt.
    #[arg(long, global = true)]
    pub(crate) passphrase_stdin: bool,

    /// For asynchronous operations (`z_sendmany`, `z_shieldcoinbase`), print the operation
    /// id immediately instead of waiting for the operation to finish.
    #[arg(long, global = true)]
    pub(crate) no_wait: bool,

    /// The JSON-RPC method to call, with its arguments.
    #[command(subcommand)]
    pub(crate) method: QueryMethodCmd,
}

/// The set of JSON-RPC methods callable via `zallet query`.
///
/// Each variant maps to a single JSON-RPC method, with typed arguments that are
/// assembled into the JSON-RPC request parameters.
#[cfg(feature = "query")]
#[derive(Debug, Subcommand)]
pub(crate) enum QueryMethodCmd {
    /// Returns wallet status information.
    #[command(
        name = "getwalletstatus",
        after_help = "Examples:\n  zallet query getwalletstatus"
    )]
    GetWalletStatus,

    /// List accounts created with z_getnewaccount or z_recoveraccounts.
    #[command(
        name = "z_listaccounts",
        after_help = "Examples:\n  zallet query z_listaccounts"
    )]
    ZListAccounts {
        /// Also include the addresses known to the wallet for each account.
        #[arg(long)]
        include_addresses: Option<bool>,
    },

    /// Returns details about the given account.
    #[command(
        name = "z_getaccount",
        after_help = "Examples:\n  \
            zallet query z_getaccount 2b8c4f1e-9a3d-4e7b-8c1f-0a2b3c4d5e6f"
    )]
    ZGetAccount {
        /// The UUID of the wallet account.
        account_uuid: String,
    },

    /// Derive a Unified Address for an account.
    #[command(
        name = "z_getaddressforaccount",
        after_help = "Examples:\n  \
            zallet query z_getaddressforaccount 2b8c4f1e-9a3d-4e7b-8c1f-0a2b3c4d5e6f\n  \
            zallet query z_getaddressforaccount 2b8c4f1e-9a3d-4e7b-8c1f-0a2b3c4d5e6f \
            --receiver-type p2pkh --receiver-type orchard"
    )]
    ZGetAddressForAccount {
        /// The account UUID, or legacy account number, to derive an address for.
        account: String,

        /// Receiver types to include (e.g. `p2pkh`, `sapling`, `orchard`).
        ///
        /// May be repeated. If omitted, a default set is used.
        #[arg(long = "receiver-type")]
        receiver_types: Vec<String>,

        /// The diversifier index to derive the address at.
        #[arg(long)]
        diversifier_index: Option<u128>,
    },

    /// Lists the addresses managed by this wallet by source.
    #[command(
        name = "listaddresses",
        after_help = "Examples:\n  zallet query listaddresses"
    )]
    ListAddresses,

    /// List the receivers within a unified address.
    #[command(
        name = "z_listunifiedreceivers",
        after_help = "Examples:\n  \
            zallet query z_listunifiedreceivers u1l9f5q2d8x7c3v4b5n6m7..."
    )]
    ZListUnifiedReceivers {
        /// The unified address to inspect.
        unified_address: String,
    },

    /// List the wallet's transactions.
    #[command(
        name = "z_listtransactions",
        after_help = "Examples:\n  \
            zallet query z_listtransactions \
            --account-uuid 2b8c4f1e-9a3d-4e7b-8c1f-0a2b3c4d5e6f\n  \
            zallet query z_listtransactions \
            --account-uuid 2b8c4f1e-9a3d-4e7b-8c1f-0a2b3c4d5e6f --limit 20 --offset 0"
    )]
    ZListTransactions {
        /// The UUID of the wallet account. If omitted, return for all accounts.
        #[arg(long)]
        account_uuid: Option<String>,

        /// Inclusive lower bound of block heights to return transactions for.
        #[arg(long)]
        start_height: Option<u32>,

        /// Exclusive upper bound of block heights to return transactions for.
        #[arg(long)]
        end_height: Option<u32>,

        /// Number of transactions to skip before a page of results is returned.
        #[arg(long)]
        offset: Option<u32>,

        /// Upper bound on the number of results returned in a page.
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Returns the raw transaction data for a transaction ID.
    #[command(
        name = "getrawtransaction",
        after_help = "Examples:\n  \
            zallet query getrawtransaction \
            a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f901\n  \
            zallet query getrawtransaction \
            a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f901 --verbose 1"
    )]
    GetRawTransaction {
        /// The transaction ID.
        txid: String,

        /// If 0, return hex-encoded data. If non-zero, return a JSON object.
        #[arg(long)]
        verbose: Option<u64>,

        /// The block in which to look for the transaction.
        #[arg(long)]
        blockhash: Option<String>,
    },

    /// Decode a hex-encoded transaction.
    #[command(
        name = "decoderawtransaction",
        after_help = "Examples:\n  \
            zallet query decoderawtransaction 0400008085202f8901...00000000"
    )]
    DecodeRawTransaction {
        /// The transaction hex string.
        hexstring: String,
    },

    /// Detailed information about an in-wallet transaction.
    #[command(
        name = "z_viewtransaction",
        after_help = "Examples:\n  \
            zallet query z_viewtransaction \
            a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f901"
    )]
    ZViewTransaction {
        /// The transaction ID.
        txid: String,
    },

    /// Validate a transparent Zcash address.
    #[command(
        name = "validateaddress",
        after_help = "Examples:\n  \
            zallet query validateaddress t1R8h9c2e3f4g5h6j7k8l9m0n1p2q3r4s5t"
    )]
    ValidateAddress {
        /// The transparent address to validate.
        address: String,
    },

    /// Verify a signed message.
    #[command(
        name = "verifymessage",
        after_help = "Examples:\n  \
            zallet query verifymessage t1R8h9c2e3f4g5h6j7k8l9m0n1p2q3r4s5t \
            H1aBcD2eFg...== \"hello world\""
    )]
    VerifyMessage {
        /// The Zcash transparent address used to sign the message.
        zcashaddress: String,

        /// The signature provided by the signer in base64 encoding.
        signature: String,

        /// The message that was signed.
        message: String,
    },

    /// Convert a transparent P2PKH address to a TEX address.
    #[command(
        name = "z_converttex",
        after_help = "Examples:\n  \
            zallet query z_converttex t1R8h9c2e3f4g5h6j7k8l9m0n1p2q3r4s5t"
    )]
    ZConvertTex {
        /// The transparent P2PKH address to convert.
        transparent_address: String,
    },

    /// Decode a hex-encoded script.
    #[command(
        name = "decodescript",
        after_help = "Examples:\n  zallet query decodescript 76a914a1b2c3...88ac"
    )]
    DecodeScript {
        /// The hex-encoded script.
        hexstring: String,
    },

    /// List all commands, or get help for a specified command.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "help",
        after_help = "Examples:\n  \
            zallet query help\n  \
            zallet query help z_sendmany"
    )]
    Help {
        /// The command to get help on.
        command: Option<String>,
    },

    /// List operation ids currently known to the wallet.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_listoperationids",
        after_help = "Examples:\n  \
            zallet query z_listoperationids\n  \
            zallet query z_listoperationids --status success"
    )]
    ZListOperationIds {
        /// Filter results by the operation's state (e.g. `success`).
        #[arg(long)]
        status: Option<String>,
    },

    /// Get operation status without removing it.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_getoperationstatus",
        after_help = "Examples:\n  \
            zallet query z_getoperationstatus \
            --operationid opid-3f2504e0-4f89-41d3-9a0c-0305e82c3301"
    )]
    ZGetOperationStatus {
        /// Operation ids to query. May be repeated.
        #[arg(long = "operationid")]
        operationid: Vec<String>,
    },

    /// Retrieve and remove finished operation results.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_getoperationresult",
        after_help = "Examples:\n  \
            zallet query z_getoperationresult \
            --operationid opid-3f2504e0-4f89-41d3-9a0c-0305e82c3301"
    )]
    ZGetOperationResult {
        /// Operation ids to retrieve. May be repeated.
        #[arg(long = "operationid")]
        operationid: Vec<String>,
    },

    /// Returns wallet state information.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "getwalletinfo",
        after_help = "Examples:\n  zallet query getwalletinfo"
    )]
    GetWalletInfo,

    /// Unlock the wallet for `timeout` seconds.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "walletpassphrase",
        after_help = "Examples:\n  zallet query walletpassphrase \"my secret passphrase\" 600"
    )]
    WalletPassphrase {
        /// The wallet passphrase.
        passphrase: String,

        /// The number of seconds to keep the wallet unlocked.
        timeout: u64,
    },

    /// Lock the wallet, removing the decryption key from memory.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "walletlock",
        after_help = "Examples:\n  zallet query walletlock"
    )]
    WalletLock,

    /// Prepare and return a new account.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_getnewaccount",
        after_help = "Examples:\n  zallet query z_getnewaccount \"My account\""
    )]
    ZGetNewAccount {
        /// The name for the new account.
        account_name: String,

        /// The seed fingerprint to derive the account from.
        #[arg(long)]
        seedfp: Option<String>,
    },

    /// Returns the balances for each spending authority.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_getbalances",
        after_help = "Examples:\n  \
            zallet query z_getbalances\n  \
            zallet query z_getbalances --minconf 1"
    )]
    ZGetBalances {
        /// Only include unspent outputs confirmed at least this many times.
        #[arg(long)]
        minconf: Option<u32>,
    },

    /// Import a transparent address into the wallet.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_importaddress",
        after_help = "Examples:\n  \
            zallet query z_importaddress 2b8c4f1e-9a3d-4e7b-8c1f-0a2b3c4d5e6f \
            02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc"
    )]
    ZImportAddress {
        /// The account UUID.
        account: String,

        /// Hex-encoded public key or redeem script.
        hex_data: String,

        /// If true, rescan the chain for UTXOs after importing.
        #[arg(long)]
        rescan: Option<bool>,
    },

    /// Returns the total value of funds in the wallet.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_gettotalbalance",
        after_help = "Examples:\n  \
            zallet query z_gettotalbalance\n  \
            zallet query z_gettotalbalance --minconf 1"
    )]
    ZGetTotalBalance {
        /// Only include transactions confirmed at least this many times.
        #[arg(long)]
        minconf: Option<u32>,

        /// Also include balance in watchonly addresses.
        #[arg(long)]
        include_watchonly: Option<bool>,
    },

    /// List unspent shielded notes.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_listunspent",
        after_help = "Examples:\n  \
            zallet query z_listunspent\n  \
            zallet query z_listunspent --minconf 1 --maxconf 9999999"
    )]
    ZListUnspent {
        /// Select outputs with at least this many confirmations.
        #[arg(long)]
        minconf: Option<u32>,

        /// Select outputs with at most this many confirmations.
        #[arg(long)]
        maxconf: Option<u32>,

        /// Include notes/utxos without spending capability.
        #[arg(long)]
        include_watchonly: Option<bool>,

        /// Addresses to retrieve UTXOs for. May be repeated.
        #[arg(long = "address")]
        addresses: Vec<String>,

        /// Execute the query as if at this blockchain height.
        #[arg(long)]
        as_of_height: Option<i64>,
    },

    /// Returns the number of notes per shielded value pool.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_getnotescount",
        after_help = "Examples:\n  \
            zallet query z_getnotescount\n  \
            zallet query z_getnotescount --minconf 1"
    )]
    ZGetNotesCount {
        /// Only include notes confirmed at least this many times.
        #[arg(long)]
        minconf: Option<u32>,

        /// Execute the query as if at this blockchain height.
        #[arg(long)]
        as_of_height: Option<i64>,
    },

    /// Tells the wallet to track a specific account.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_recoveraccounts",
        after_help = "Examples:\n  \
            zallet query z_recoveraccounts \
            --name \"Recovered account\" \
            --seedfp 0f6d2c1a9b3e4d5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7 \
            --zip32-account-index 0 \
            --birthday-height 2800000"
    )]
    ZRecoverAccounts {
        /// A human-readable name for the account.
        #[arg(long)]
        name: String,

        /// The seed fingerprint (hex) of the mnemonic the account is derived from.
        #[arg(long)]
        seedfp: String,

        /// The ZIP 32 account index.
        #[arg(long)]
        zip32_account_index: u32,

        /// The block height at which the account was created.
        #[arg(long)]
        birthday_height: u32,
    },

    /// Send a transaction with one or more recipients.
    ///
    /// Each recipient is specified by a `--to` address and an `--amount`, with an optional
    /// `--memo`. To send to multiple recipients, repeat the `--to`/`--amount` pair; the Nth
    /// `--to` is paired with the Nth `--amount` (and the Nth `--memo`, if any are given).
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_sendmany",
        after_help = "Examples:\n  \
            zallet query z_sendmany u1qw508d6qejxtdg4y5r3zarvary0c5xw7k... \
            --to u1l9f5q2d8x7c... --amount 1.5\n  \
            zallet query z_sendmany u1qw508d6qejxtdg4y5r3zarvary0c5xw7k... \
            --to u1l9f5q2d8x7c... --amount 1.5 \
            --to t1R8h9c2e3f4... --amount 0.25"
    )]
    ZSendMany {
        /// The transparent or shielded address to send the funds from.
        fromaddress: String,

        /// A recipient address. Repeat (paired with `--amount`) for multiple recipients.
        #[arg(long = "to")]
        to: Vec<String>,

        /// The amount in ZEC to send to the corresponding `--to` recipient.
        #[arg(long = "amount")]
        amounts: Vec<String>,

        /// An optional hex-encoded memo for the corresponding `--to` recipient (shielded
        /// recipients only). If given for any recipient, it is paired positionally with the
        /// `--to` recipients.
        #[arg(long = "memo")]
        memos: Vec<String>,

        /// Only use funds confirmed at least this many times.
        #[arg(long)]
        minconf: Option<u32>,

        /// If set, must be `null` (Zallet always uses a ZIP 317 fee).
        #[arg(long)]
        fee: Option<String>,

        /// Policy for what information leakage is acceptable.
        #[arg(long)]
        privacy_policy: Option<String>,
    },

    /// Shield coinbase UTXOs into a shielded address.
    #[cfg(zallet_build = "wallet")]
    #[command(
        name = "z_shieldcoinbase",
        after_help = "Examples:\n  \
            zallet query z_shieldcoinbase t1R8h9c2e3f4g5h6j7k8... u1l9f5q2d8x7c..."
    )]
    ZShieldCoinbase {
        /// A single wallet-owned transparent address, or an account UUID.
        fromaddress: String,

        /// The shielded address that will receive the funds.
        toaddress: String,

        /// If set, must be `null` (Zallet always uses a ZIP 317 fee).
        #[arg(long)]
        fee: Option<String>,

        /// Cap the number of selected coinbase UTXOs to the highest-value `n`.
        #[arg(long)]
        limit: Option<u32>,

        /// Hex-encoded memo to store in the resulting shielded payment.
        #[arg(long)]
        memo: Option<String>,

        /// Policy for what information leakage is acceptable.
        #[arg(long)]
        privacy_policy: Option<String>,
    },
}

#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command, Runnable))]
pub(crate) enum RepairCmd {
    TruncateWallet(TruncateWalletCmd),
}

/// Truncates the wallet database to at most the specified height.
///
/// Upon successful truncation, this method returns the height to which the data store was
/// actually truncated. `zallet start` will then sync the wallet as if this height was the
/// last observed chain tip height.
///
/// There may be restrictions on heights to which it is possible to truncate.
/// Specifically, it will only be possible to truncate to heights at which is is possible
/// to create a witness given the current state of the wallet's note commitment tree.
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct TruncateWalletCmd {
    /// The maximum height the wallet may treat as a mined block.
    ///
    /// Zallet may choose a lower block height to which the data store will be truncated
    /// if it is not possible to truncate exactly to the specified height.
    pub(crate) max_height: u32,
}

#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command, Runnable))]
pub(crate) enum RegtestCmd {
    /// Generate a default account and return a transparent address for miner outputs.
    #[cfg(zallet_build = "wallet")]
    GenerateAccountAndMinerAddress(GenerateAccountAndMinerAddressCmd),
}

#[cfg(zallet_build = "wallet")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct GenerateAccountAndMinerAddressCmd {}
