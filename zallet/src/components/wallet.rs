use std::fmt;
use std::path::Path;
use std::time::Duration;

use abscissa_core::{tracing::info, Component, FrameworkError};
use abscissa_tokio::TokioComponent;
use tokio::{fs, task::JoinHandle, time};
use zcash_client_backend::sync;
use zcash_client_sqlite::wallet::init::{init_wallet_db, WalletMigrationError};

use crate::{
    error::{Error, ErrorKind},
    network::Network,
    remote::Servers,
};

mod cache;

mod connection;
pub(crate) use connection::WalletConnection;

pub(crate) type WalletHandle = deadpool::managed::Object<connection::WalletManager>;

#[derive(Clone, Component)]
#[component(inject = "init_tokio(abscissa_tokio::TokioComponent)")]
pub(crate) struct Wallet {
    params: Network,
    db_data_pool: connection::WalletPool,
    lightwalletd_server: Servers,
}

impl fmt::Debug for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wallet")
            .field("params", &self.params)
            .field("lightwalletd_server", &self.lightwalletd_server)
            .finish_non_exhaustive()
    }
}

impl Wallet {
    pub async fn open(
        path: impl AsRef<Path>,
        params: Network,
        lightwalletd_server: Servers,
    ) -> Result<Self, Error> {
        let wallet_exists = fs::try_exists(&path)
            .await
            .map_err(|e| ErrorKind::Init.context(e))?;

        let wallet = Self {
            params,
            db_data_pool: connection::pool(path, params)?,
            lightwalletd_server,
        };

        if wallet_exists {
            info!("Applying latest database migrations");
        } else {
            info!("Creating empty database");
        }
        let handle = wallet.handle().await?;
        handle.with_mut(|mut db_data| {
            match init_wallet_db(&mut db_data, None) {
                Ok(()) => Ok(()),
                // TODO: Support single-seed migrations once we have key storage.
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

        Ok(wallet)
    }

    /// Called automatically after `TokioComponent` is initialized
    pub fn init_tokio(&mut self, _tokio_cmp: &TokioComponent) -> Result<(), FrameworkError> {
        Ok(())
    }

    pub(crate) async fn handle(&self) -> Result<WalletHandle, Error> {
        self.db_data_pool
            .get()
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }

    pub async fn spawn_sync(&self) -> Result<JoinHandle<Result<(), Error>>, Error> {
        let mut client = self
            .lightwalletd_server
            .pick(self.params)?
            .connect_direct()
            .await?;

        let params = self.params.clone();

        let mut db_cache = cache::MemoryCache::new();

        let mut db_data = self.handle().await?;

        let mut interval = time::interval(Duration::from_secs(30));

        let task = tokio::spawn(async move {
            loop {
                // TODO: Move this inside `sync::run` so that we aren't querying subtree roots
                // every interval.
                interval.tick().await;

                sync::run(
                    &mut client,
                    &params,
                    &mut db_cache,
                    db_data.as_mut(),
                    10_000,
                )
                .await
                .map_err(|e| ErrorKind::Generic.context(e))?;
            }
        });

        Ok(task)
    }
}
