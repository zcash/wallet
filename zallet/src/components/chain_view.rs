use std::fmt;
use std::sync::Arc;

use jsonrpsee::tracing::info;
use tokio::sync::RwLock;
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
pub(crate) struct ChainView {
    // TODO: Migrate to `StateService`.
    indexer: Arc<RwLock<Option<IndexerService<FetchService>>>>,
}

impl fmt::Debug for ChainView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainView").finish_non_exhaustive()
    }
}

impl ChainView {
    pub(crate) async fn new(config: &ZalletConfig) -> Result<(Self, TaskHandle), Error> {
        let validator_rpc_address =
            config
                .indexer
                .validator_address
                .unwrap_or_else(|| match config.network() {
                    crate::network::Network::Consensus(
                        zcash_protocol::consensus::Network::MainNetwork,
                    ) => "127.0.0.1:8232".parse().unwrap(),
                    _ => "127.0.0.1:18232".parse().unwrap(),
                });

        let db_path = config
            .indexer
            .db_path
            .clone()
            .ok_or(ErrorKind::Init.context("indexer.db_path must be set (for now)"))?;

        let config = FetchServiceConfig::new(
            validator_rpc_address,
            config.indexer.validator_cookie_auth.unwrap_or(false),
            config.indexer.validator_cookie_path.clone(),
            config.indexer.validator_user.clone(),
            config.indexer.validator_password.clone(),
            None,
            None,
            None,
            None,
            db_path,
            None,
            config.network().to_zebra(),
            false,
            // Setting this to `false` causes start-up to block on completely filling the
            // cache. Zaino's DB currently only contains a cache of CompactBlocks, so we
            // make do for now with uncached queries.
            // TODO: https://github.com/zingolabs/zaino/issues/249
            true,
        );

        info!("Starting Zaino indexer");
        let indexer = Arc::new(RwLock::new(Some(
            IndexerService::<FetchService>::spawn(config)
                .await
                .map_err(|e| ErrorKind::Init.context(e))?,
        )));

        let chain_view = Self {
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

        Ok((chain_view, task))
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
