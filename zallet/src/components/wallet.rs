use std::fmt;
use std::path::Path;
use std::time::Duration;

use abscissa_core::{Component, FrameworkError};
use abscissa_tokio::TokioComponent;
use tokio::{task::JoinHandle, time};
use zcash_client_backend::sync;

use crate::{
    error::{Error, ErrorKind},
    network::Network,
    remote::Servers,
};

mod cache;
mod connection;

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
    pub fn open(
        path: impl AsRef<Path>,
        params: Network,
        lightwalletd_server: Servers,
    ) -> Result<Self, Error> {
        let db_data_pool = connection::pool(path, params)?;
        Ok(Self {
            params,
            db_data_pool,
            lightwalletd_server,
        })
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
