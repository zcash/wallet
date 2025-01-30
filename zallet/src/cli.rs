use std::path::PathBuf;

use abscissa_core::{Command, Runnable};
use clap::{builder::Styles, Parser};

use crate::{fl, remote::Servers};

#[derive(Debug, Parser, Command)]
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

    /// Use the specified config file
    #[arg(short, long)]
    pub(crate) config: Option<String>,
}

#[derive(Debug, Parser, Command, Runnable)]
pub(crate) enum ZalletCmd {
    /// The `start` subcommand
    Start(StartCmd),

    /// Generate a `zallet.toml` config from an existing `zcashd.conf` file.
    MigrateZcashdConf(MigrateZcashConfCmd),
}

/// `start` subcommand
#[derive(Debug, Parser, Command)]
pub(crate) struct StartCmd {
    /// The lightwalletd server to sync with (default is \"ecc\")
    #[arg(long)]
    #[arg(default_value = "ecc", value_parser = Servers::parse)]
    pub(crate) lwd_server: Servers,
}

/// `migrate-zcash-conf` subcommand
#[derive(Debug, Parser, Command)]
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
