//! Zallet Subcommands

use std::{
    fs,
    path::{Path, PathBuf},
};

use abscissa_core::{
    Application, Configurable, FrameworkError, FrameworkErrorKind, Runnable, Shutdown,
    config::Override,
};
use home::home_dir;
use tracing::info;

use crate::{
    cli::{EntryPoint, ZalletCmd},
    config::ZalletConfig,
    error::{Error, ErrorKind},
    fl,
    prelude::APP,
};

mod add_rpc_user;
mod example_config;
mod repair;
mod start;

#[cfg(zallet_build = "wallet")]
mod export_mnemonic;
#[cfg(zallet_build = "wallet")]
mod generate_mnemonic;
#[cfg(zallet_build = "wallet")]
mod import_mnemonic;
#[cfg(zallet_build = "wallet")]
mod init_wallet_encryption;
#[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
mod migrate_zcash_conf;
#[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
mod migrate_zcashd_wallet;

#[cfg(feature = "rpc-cli")]
pub(crate) mod rpc_cli;

/// Zallet Configuration Filename
pub const CONFIG_FILE: &str = "zallet.toml";

/// Ensures only a single Zallet process is using the data directory.
pub(crate) fn lock_datadir(datadir: &Path) -> Result<fmutex::Guard<'static>, Error> {
    let lockfile_path = resolve_datadir_path(datadir, Path::new(".lock"));

    {
        // Ensure that the lockfile exists on disk.
        let _ = fs::File::create(&lockfile_path).map_err(|e| {
            ErrorKind::Init.context(fl!(
                "err-init-failed-to-create-lockfile",
                path = lockfile_path.display().to_string(),
                error = e.to_string(),
            ))
        })?;
    }

    let guard = fmutex::try_lock_exclusive_path(&lockfile_path)
        .map_err(|e| {
            ErrorKind::Init.context(fl!(
                "err-init-failed-to-read-lockfile",
                path = lockfile_path.display().to_string(),
                error = e.to_string(),
            ))
        })?
        .ok_or_else(|| {
            ErrorKind::Init.context(fl!(
                "err-init-zallet-already-running",
                datadir = datadir.display().to_string(),
            ))
        })?;

    Ok(guard)
}

/// Resolves the requested path relative to the Zallet data directory.
pub(crate) fn resolve_datadir_path(datadir: &Path, path: &Path) -> PathBuf {
    // TODO: Do we canonicalize here? Where do we enforce any requirements on the
    //       config's relative paths?
    //       https://github.com/zcash/wallet/issues/249
    datadir.join(path)
}

impl EntryPoint {
    /// Returns the data directory to use for this Zallet command.
    fn datadir(&self) -> Result<PathBuf, FrameworkError> {
        // TODO: Decide whether to make either the default datadir, or every datadir,
        //       chain-specific.
        //       https://github.com/zcash/wallet/issues/250
        if let Some(datadir) = &self.datadir {
            Ok(datadir.clone())
        } else {
            // The XDG Base Directory Specification is widely misread as saying that
            // `$XDG_DATA_HOME` should be used for storing mutable user-generated data.
            // The specification actually says that it is the userspace version of
            // `/usr/share` and is for user-specific versions of the latter's files. And
            // per the Filesystem Hierarchy Standard:
            //
            // > The `/usr/share` hierarchy is for all read-only architecture independent
            // > data files.
            //
            // This has led to inconsistent beliefs about which of `$XDG_CONFIG_HOME` and
            // `$XDG_DATA_HOME` should be backed up, and which is safe to delete at any
            // time. See https://bsky.app/profile/str4d.xyz/post/3lsjbnpsbh22i for more
            // details.
            //
            // Given the above, we eschew the XDG Base Directory Specification entirely,
            // and use `$HOME/.zallet` as the default datadir. The config file provides
            // sufficient flexibility for individual users to use XDG paths at their own
            // risk (and with knowledge of their OS environment's behaviour).
            home_dir()
                .ok_or_else(|| {
                    FrameworkErrorKind::ComponentError
                        .context(fl!("err-init-cannot-find-home-dir"))
                        .into()
                })
                .map(|base| base.join(".zallet"))
        }
    }
}

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
        let filename = resolve_datadir_path(
            &self.datadir().ok()?,
            self.config
                .as_deref()
                .unwrap_or_else(|| Path::new(CONFIG_FILE)),
        );

        if filename.exists() {
            Some(filename)
        } else {
            None
        }
    }

    fn process_config(&self, mut config: ZalletConfig) -> Result<ZalletConfig, FrameworkError> {
        // Components access top-level CLI settings solely through `ZalletConfig`.
        // Load them in here.
        config.datadir = Some(self.datadir()?);

        match &self.cmd {
            ZalletCmd::Start(cmd) => cmd.override_config(config),
            _ => Ok(config),
        }
    }
}

/// An async version of the [`Runnable`] trait.
pub(crate) trait AsyncRunnable {
    /// Runs this `AsyncRunnable`.
    async fn run(&self) -> Result<(), Error>;

    /// Runs this `AsyncRunnable` using the `abscissa_tokio` runtime.
    ///
    /// Signal detection is included for handling both interrupts (Ctrl-C on most
    /// platforms, corresponding to `SIGINT` on Unix), and programmatic termination
    /// (`SIGTERM` on Unix). Both of these will cause [`AsyncRunnable::run`] to be
    /// cancelled (ending execution at an `.await` boundary).
    ///
    /// This should be called from [`Runnable::run`].
    fn run_on_runtime(&self) {
        match abscissa_tokio::run(&APP, async move {
            tokio::select! {
                biased;
                _ = shutdown() => Ok(()),
                result = self.run() => result,
            }
        }) {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                eprintln!("{e}");
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
            Err(e) => {
                eprintln!("{e}");
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
        }
    }
}

async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint =
            signal(SignalKind::interrupt()).expect("Failed to register signal handler for SIGINT");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to register signal handler for SIGTERM");

        let signal = tokio::select! {
            _ = sigint.recv() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        };

        info!("Received {signal}, starting shutdown");
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("listening for ctrl-c signal should never fail");

        info!("Received Ctrl-C, starting shutdown");
    }
}
