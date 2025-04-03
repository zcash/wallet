use std::time::Duration;

use tokio::{task::JoinHandle, time};
use zcash_client_backend::sync;

use crate::{
    config::ZalletConfig,
    error::{Error, ErrorKind},
    remote::Servers,
};

use super::database::Database;

mod cache;

#[derive(Debug)]
pub(crate) struct WalletSync {}

impl WalletSync {
    pub(crate) async fn spawn(
        config: &ZalletConfig,
        db: Database,
        lightwalletd_server: Servers,
    ) -> JoinHandle<Result<(), Error>> {
        let params = config.network();
        let db_cache = cache::MemoryCache::new();

        tokio::spawn(async move {
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
        })
    }
}
