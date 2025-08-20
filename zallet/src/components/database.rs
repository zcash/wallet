use std::fmt;

use abscissa_core::tracing::info;
use schemerz_rusqlite::RusqliteMigration;
use tokio::fs;
use zcash_client_sqlite::wallet::init::{WalletMigrationError, WalletMigrator};

use crate::{
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

use super::keystore;

mod connection;
pub(crate) use connection::DbConnection;

#[cfg(test)]
mod tests;

pub(crate) type DbHandle = deadpool::managed::Object<connection::WalletManager>;

/// Returns the full list of migrations defined in Zallet, to be applied alongside the
/// migrations internal to `zcash_client_sqlite`.
fn all_external_migrations() -> Vec<Box<dyn RusqliteMigration<Error = WalletMigrationError>>> {
    keystore::db::migrations::all().collect()
}

#[derive(Clone)]
pub(crate) struct Database {
    db_data_pool: connection::WalletPool,
}

impl fmt::Debug for Database {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Database").finish_non_exhaustive()
    }
}

impl Database {
    pub(crate) async fn open(config: &ZalletConfig) -> Result<Self, Error> {
        let path = config.wallet_db_path();

        let db_exists = fs::try_exists(&path)
            .await
            .map_err(|e| ErrorKind::Init.context(e))?;

        let db_data_pool = connection::pool(&path, config.consensus.network())?;

        let database = Self { db_data_pool };

        // Initialize the database before we go any further.
        if db_exists {
            info!("Applying latest database migrations");
        } else {
            info!("Creating empty database");
        }
        let handle = database.handle().await?;
        handle.with_mut(|mut db_data| {
            match WalletMigrator::new()
                .with_external_migrations(all_external_migrations())
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
                }) => Err(ErrorKind::Init.context("TODO: Support seed-required migrations")),
                Err(e) => Err(ErrorKind::Init.context(e)),
            }?;

            Ok::<(), Error>(())
        })?;

        Ok(database)
    }

    pub(crate) async fn handle(&self) -> Result<DbHandle, Error> {
        self.db_data_pool
            .get()
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }
}
