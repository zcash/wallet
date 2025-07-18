use std::path::PathBuf;

use clap::{Parser, builder::Styles};

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
    MigrateZcashConf(MigrateZcashConfCmd),

    /// Initialize wallet encryption.
    InitWalletEncryption(InitWalletEncryptionCmd),

    /// Generate a BIP 39 mnemonic phrase and store it in the wallet.
    GenerateMnemonic(GenerateMnemonicCmd),

    /// Import a BIP 39 mnemonic phrase into the wallet.
    ImportMnemonic(ImportMnemonicCmd),

    /// Communicate with a Zallet wallet's JSON-RPC interface.
    #[cfg(feature = "rpc-cli")]
    Rpc(RpcCliCmd),
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
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct MigrateZcashConfCmd {
    /// Specify `zcashd` configuration file.
    ///
    /// Relative paths will be prefixed by `datadir` location.
    #[arg(long, default_value = "zcash.conf")]
    pub(crate) conf: PathBuf,

    /// Specify `zcashd` data directory (this path cannot use '~').
    #[arg(long)]
    pub(crate) datadir: Option<PathBuf>,

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

/// `init-wallet-encryption` subcommand
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct InitWalletEncryptionCmd {}

/// `generate-mnemonic` subcommand
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct GenerateMnemonicCmd {}

/// `import-mnemonic` subcommand
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct ImportMnemonicCmd {}

/// `rpc` subcommand
#[cfg(feature = "rpc-cli")]
#[derive(Debug, Parser)]
#[cfg_attr(outside_buildscript, derive(Command))]
pub(crate) struct RpcCliCmd {
    /// The JSON-RPC command to send to Zallet.
    pub(crate) command: String,

    /// Any parameters for the command.
    pub(crate) params: Vec<String>,
}
