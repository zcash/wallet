use std::fmt;

use abscissa_core::tracing::info;
use rusqlite::{OptionalExtension, named_params};
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

mod ext;

#[cfg(test)]
mod tests;

pub(crate) type DbHandle = deadpool::managed::Object<connection::WalletManager>;

/// Returns the full list of migrations defined in Zallet, to be applied alongside the
/// migrations internal to `zcash_client_sqlite`.
fn all_external_migrations() -> Vec<Box<dyn RusqliteMigration<Error = WalletMigrationError>>> {
    ext::migrations::all()
        .chain(keystore::db::migrations::all())
        .collect()
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

        let now = ::time::OffsetDateTime::now_utc();

        // Record that we migrated the database using this Zallet version. We don't have
        // an easy way to detect whether any migrations actually ran, so we check whether
        // the most recent entry matches the current version tuple, and only record an
        // entry if it doesn't.
        handle.with_raw_mut(|conn| {
            #[allow(clippy::const_is_empty)]
            let (git_revision, clean) = (!crate::build::COMMIT_HASH.is_empty())
                .then_some((crate::build::COMMIT_HASH, crate::build::GIT_CLEAN))
                .unzip();

            match conn
                .query_row(
                    "SELECT version, git_revision, clean
                    FROM ext_zallet_db_version_metadata
                    ORDER BY rowid DESC
                    LIMIT 1",
                    [],
                    |row| {
                        Ok(
                            row.get::<_, String>("version")? == crate::build::PKG_VERSION
                                && row.get::<_, Option<String>>("git_revision")?.as_deref()
                                    == git_revision
                                && row.get::<_, Option<bool>>("clean")? == clean,
                        )
                    },
                )
                .optional()
                .map_err(|e| ErrorKind::Init.context(e))?
            {
                Some(true) => (),
                None | Some(false) => {
                    conn.execute(
                        "INSERT INTO ext_zallet_db_version_metadata
                        VALUES (:version, :git_revision, :clean, :migrated)",
                        named_params! {
                            ":version": crate::build::PKG_VERSION,
                            ":git_revision": git_revision,
                            ":clean": clean,
                            ":migrated": now,
                        },
                    )
                    .map_err(|e| ErrorKind::Init.context(e))?;
                }
            }

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
