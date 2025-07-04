//! Zallet Subcommands

use std::path::PathBuf;

use abscissa_core::{Configurable, FrameworkError, Runnable, config::Override};

use crate::{
    cli::{EntryPoint, ZalletCmd},
    config::ZalletConfig,
};

mod example_config;
mod generate_mnemonic;
mod import_mnemonic;
mod init_wallet_encryption;
mod migrate_zcash_conf;
mod start;

#[cfg(feature = "rpc-cli")]
pub(crate) mod rpc_cli;

/// Zallet Configuration Filename
pub const CONFIG_FILE: &str = "zallet.toml";

impl Runnable for EntryPoint {
    fn run(&self) {
        self.cmd.run()
    }
}

impl Configurable<ZalletConfig> for EntryPoint {
    fn config_path(&self) -> Option<PathBuf> {
        // Check if the config file exists, and if it does not, ignore it.
        // If you'd like for a missing configuration file to be a hard error
        // instead, always return `Some(CONFIG_FILE)` here.
        let filename = self
            .config
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| CONFIG_FILE.into());

        if filename.exists() {
            Some(filename)
        } else {
            None
        }
    }

    fn process_config(&self, config: ZalletConfig) -> Result<ZalletConfig, FrameworkError> {
        match &self.cmd {
            ZalletCmd::Start(cmd) => cmd.override_config(config),
            _ => Ok(config),
        }
    }
}
