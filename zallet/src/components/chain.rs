#![allow(deprecated)] // For zaino

use std::fmt;
use std::sync::Arc;

use jsonrpsee::tracing::info;
use tokio::sync::RwLock;
use zaino_common::{CacheConfig, DatabaseConfig, ServiceConfig, StorageConfig};
use zaino_state::{
    FetchService, FetchServiceConfig, FetchServiceSubscriber, IndexerService, IndexerSubscriber,
    StatusType, ZcashService,
};

use crate::{
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

use super::TaskHandle;

#[derive(Clone)]
pub(crate) struct Chain {
    // TODO: Migrate to `StateService`.
    indexer: Arc<RwLock<Option<IndexerService<FetchService>>>>,
}

impl fmt::Debug for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chain").finish_non_exhaustive()
    }
}

impl Chain {
    pub(crate) async fn new(config: &ZalletConfig) -> Result<(Self, TaskHandle), Error> {
        let validator_rpc_address =
            config
                .indexer
                .validator_address
                .as_deref()
                .unwrap_or_else(|| match config.consensus.network() {
                    crate::network::Network::Consensus(
                        zcash_protocol::consensus::Network::MainNetwork,
                    ) => "127.0.0.1:8232",
                    _ => "127.0.0.1:18232",
                });

        let config = FetchServiceConfig::new(
            validator_rpc_address.into(),
            config.indexer.validator_cookie_path.clone(),
            config.indexer.validator_user.clone(),
            config.indexer.validator_password.clone(),
            ServiceConfig::default(),
            StorageConfig {
                cache: CacheConfig::default(),
                database: DatabaseConfig {
                    path: config.indexer_db_path().to_path_buf(),
                    // Setting this to as non-zero value causes start-up to block on
                    // completely filling the cache. Zaino's DB currently only contains a
                    // cache of CompactBlocks, so we make do for now with uncached queries.
                    // TODO: https://github.com/zingolabs/zaino/issues/249
                    size: zaino_common::DatabaseSize::Gb(0),
                },
            },
            config.consensus.network().to_zaino(),
        );

        info!("Starting Zaino indexer");
        let indexer = Arc::new(RwLock::new(Some(
            IndexerService::<FetchService>::spawn(config)
                .await
                .map_err(|e| ErrorKind::Init.context(e))?,
        )));

        let chain = Self {
            indexer: indexer.clone(),
        };

        // Spawn a task that stops the indexer when appropriate internal signals occur.
        let task = crate::spawn!("Indexer shutdown", async move {
            let mut server_interval =
                tokio::time::interval(tokio::time::Duration::from_millis(100));

            loop {
                server_interval.tick().await;

                let service = indexer.read().await;
                let status = match service.as_ref() {
                    Some(service) => service.inner_ref().status().await,
                    None => StatusType::CriticalError,
                };

                // Check for errors.
                if matches!(status, StatusType::Offline | StatusType::CriticalError) {
                    let service = indexer.write().await.take().expect("only happens once");
                    service.inner().close();
                    return Err(ErrorKind::Generic.into());
                }

                // Check for shutdown signals.
                if status == StatusType::Closing {
                    let service = indexer.write().await.take().expect("only happens once");
                    service.inner().close();
                    return Ok(());
                }
            }
        });

        Ok((chain, task))
    }

    pub(crate) async fn subscribe(
        &self,
    ) -> Result<IndexerSubscriber<FetchServiceSubscriber>, Error> {
        Ok(self
            .indexer
            .read()
            .await
            .as_ref()
            .ok_or_else(|| ErrorKind::Generic.context("ChainState indexer is not running"))?
            .inner_ref()
            .get_subscriber())
    }
}
