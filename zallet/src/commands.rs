//! Zallet Subcommands

use std::path::PathBuf;

use abscissa_core::{config::Override, Configurable, FrameworkError, Runnable};

use crate::{
    cli::{EntryPoint, ZalletCmd},
    config::ZalletConfig,
};

mod migrate_zcash_conf;
mod start;

/// Zallet Configuration Filename
pub const CONFIG_FILE: &str = "zallet.toml";

impl Runnable for EntryPoint {
    fn run(&self) {
        self.cmd.run()
    }
}

impl Configurable<ZalletConfig> for EntryPoint {
    /// Location of the configuration file
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

    /// Apply changes to the config after it's been loaded, e.g. overriding
    /// values in a config file using command-line options.
    ///
    /// This can be safely deleted if you don't want to override config
    /// settings from command-line options.
    fn process_config(&self, config: ZalletConfig) -> Result<ZalletConfig, FrameworkError> {
        match &self.cmd {
            ZalletCmd::Start(cmd) => cmd.override_config(config),
            //
            // If you don't need special overrides for some
            // subcommands, you can just use a catch all
            // _ => Ok(config),
            _ => Ok(config),
        }
    }
}
