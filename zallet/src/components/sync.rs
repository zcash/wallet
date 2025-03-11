use std::fmt;
use std::time::Duration;

use abscissa_core::{component::Injectable, Component, FrameworkError};
use abscissa_tokio::TokioComponent;
use tokio::{task::JoinHandle, time};
use zcash_client_backend::sync;

use crate::{
    application::ZalletApp,
    config::ZalletConfig,
    error::{Error, ErrorKind},
    network::Network,
    remote::Servers,
};

use super::database::Database;

mod cache;

#[derive(Injectable)]
#[component(inject = "init_db(zallet::components::database::Database)")]
#[component(inject = "init_tokio(abscissa_tokio::TokioComponent)")]
pub(crate) struct WalletSync {
    params: Option<Network>,
    db: Option<Database>,
    lightwalletd_server: Servers,
    pub(crate) sync_task: Option<JoinHandle<Result<(), Error>>>,
}

impl fmt::Debug for WalletSync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalletSync")
            .field("params", &self.params)
            .field("lightwalletd_server", &self.lightwalletd_server)
            .finish_non_exhaustive()
    }
}

impl WalletSync {
    pub(crate) fn new(lightwalletd_server: Servers) -> Self {
        Self {
            params: None,
            db: None,
            lightwalletd_server,
            sync_task: None,
        }
    }
}

impl Component<ZalletApp> for WalletSync {
    fn after_config(&mut self, config: &ZalletConfig) -> Result<(), FrameworkError> {
        self.params = Some(config.network());
        Ok(())
    }
}

impl WalletSync {
    /// Called automatically after `Database` is initialized
    pub fn init_db(&mut self, db: &Database) -> Result<(), FrameworkError> {
        self.db = Some(db.clone());
        Ok(())
    }

    /// Called automatically after `TokioComponent` is initialized
    pub fn init_tokio(&mut self, tokio_cmp: &TokioComponent) -> Result<(), FrameworkError> {
        let params = self.params.expect("configured");
        let db = self.db.clone().expect("Database initialized");
        let lightwalletd_server = self.lightwalletd_server.clone();

        let db_cache = cache::MemoryCache::new();

        let runtime = tokio_cmp.runtime()?;

        let task = runtime.spawn(async move {
            let mut client = lightwalletd_server.pick(params)?.connect_direct().await?;
            let mut db_data = db.handle().await?;

            let mut interval = time::interval(Duration::from_secs(30));

            loop {
                // TODO: Move this inside `sync::run` so that we aren't querying subtree roots
                // every interval.
                interval.tick().await;

                sync::run(&mut client, &params, &db_cache, db_data.as_mut(), 10_000)
                    .await
                    .map_err(|e| ErrorKind::Generic.context(e))?;
            }
        });

        self.sync_task = Some(task);

        Ok(())
    }
}
