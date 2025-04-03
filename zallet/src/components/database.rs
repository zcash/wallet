use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use abscissa_core::{
    component::Injectable, tracing::info, Component, FrameworkError, FrameworkErrorKind,
};
use abscissa_tokio::TokioComponent;
use tokio::fs;
use zcash_client_sqlite::wallet::init::{WalletMigrationError, WalletMigrator};

use crate::{
    application::ZalletApp,
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

use super::keystore;

mod connection;
pub(crate) use connection::DbConnection;

#[cfg(test)]
mod tests;

pub(crate) type DbHandle = deadpool::managed::Object<connection::WalletManager>;

#[derive(Clone, Default, Injectable)]
#[component(inject = "init_tokio(abscissa_tokio::TokioComponent)")]
pub(crate) struct Database {
    path: Option<Arc<PathBuf>>,
    db_data_pool: Option<connection::WalletPool>,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Component<ZalletApp> for Database {
    fn after_config(&mut self, config: &ZalletConfig) -> Result<(), FrameworkError> {
        let path = config.wallet_db.clone().ok_or_else(|| {
            FrameworkErrorKind::ComponentError
                .context(ErrorKind::Init.context("wallet_db must be set (for now)"))
        })?;
        if path.is_relative() {
            return Err(FrameworkErrorKind::ComponentError
                .context(ErrorKind::Init.context("wallet_db must be an absolute path (for now)"))
                .into());
        }

        self.db_data_pool = Some(
            connection::pool(&path, config.network())
                .map_err(|e| FrameworkErrorKind::ComponentError.context(e))?,
        );
        self.path = Some(Arc::new(path));

        Ok(())
    }
}

impl Database {
    /// Called automatically after `TokioComponent` is initialized
    pub fn init_tokio(&mut self, tokio_cmp: &TokioComponent) -> Result<(), FrameworkError> {
        let runtime = tokio_cmp.runtime()?;

        // Initialize the database before we go any further.
        runtime
            .block_on(async {
                let path = self.path.as_ref().expect("configured");

                let db_exists = fs::try_exists(path.as_ref())
                    .await
                    .map_err(|e| ErrorKind::Init.context(e))?;

                if db_exists {
                    info!("Applying latest database migrations");
                } else {
                    info!("Creating empty database");
                }
                let handle = self.handle().await?;
                handle.with_mut(|mut db_data| {
                    match WalletMigrator::new()
                        .with_external_migrations(keystore::db::migrations::all())
                        .init_or_migrate(&mut db_data)
                    {
                        Ok(()) => Ok(()),
                        // TODO: KeyStore depends on Database, but we haven't finished
                        // initializing both yet. We might need to write logic to either
                        // defer initialization until later, or expose enough of the
                        // keystore read logic to let us parse the keystore database here
                        // before the KeyStore component is initialized.
                        //       https://github.com/zcash/wallet/issues/18
                        // TODO: Support multi-seed or seed-absent migrations.
                        //       https://github.com/zcash/librustzcash/issues/1284
                        Err(schemerz::MigratorError::Migration {
                            error: WalletMigrationError::SeedRequired,
                            ..
                        }) => {
                            Err(ErrorKind::Init.context("TODO: Support seed-required migrations"))
                        }
                        Err(e) => Err(ErrorKind::Init.context(e)),
                    }?;

                    Ok::<(), Error>(())
                })?;

                Ok::<_, Error>(())
            })
            .map_err(|e| FrameworkErrorKind::ComponentError.context(e))?;

        Ok(())
    }

    pub(crate) async fn handle(&self) -> Result<DbHandle, Error> {
        self.db_data_pool
            .as_ref()
            .ok_or_else(|| {
                ErrorKind::Init
                    .context("Database component must be configured before calling `handle`")
            })?
            .get()
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }
}
