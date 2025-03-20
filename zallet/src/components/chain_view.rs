use std::fmt;
use std::sync::Arc;

use abscissa_core::{component::Injectable, Component, FrameworkError, FrameworkErrorKind};
use abscissa_tokio::TokioComponent;
use jsonrpsee::tracing::info;
use tokio::{
    sync::{watch, RwLock},
    task::JoinHandle,
};
use zaino_state::{
    config::FetchServiceConfig,
    fetch::{FetchService, FetchServiceSubscriber},
    indexer::{IndexerService, IndexerSubscriber, ZcashService},
    status::StatusType,
};

use crate::{
    application::ZalletApp,
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

#[derive(Injectable)]
#[component(inject = "init_tokio(abscissa_tokio::TokioComponent)")]
pub(crate) struct ChainView {
    config: Option<FetchServiceConfig>,
    // TODO: Migrate to `StateService`.
    indexer: Arc<RwLock<Option<IndexerService<FetchService>>>>,
    started_tx: Option<watch::Sender<bool>>,
    started_rx: watch::Receiver<bool>,
    pub(crate) serve_task: Option<JoinHandle<Result<(), Error>>>,
}

impl Default for ChainView {
    fn default() -> Self {
        let (started_tx, started_rx) = watch::channel(false);
        Self {
            config: Default::default(),
            indexer: Default::default(),
            started_tx: Some(started_tx),
            started_rx,
            serve_task: Default::default(),
        }
    }
}

impl Clone for ChainView {
    fn clone(&self) -> Self {
        // We only care about cloning the indexer handle; the other fields are temporary
        // holders that have their contents taken during initialization.
        Self {
            config: None,
            indexer: self.indexer.clone(),
            started_tx: None,
            started_rx: self.started_rx.clone(),
            serve_task: None,
        }
    }
}

impl fmt::Debug for ChainView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainView").finish_non_exhaustive()
    }
}

impl Component<ZalletApp> for ChainView {
    fn after_config(&mut self, config: &ZalletConfig) -> Result<(), FrameworkError> {
        let validator_rpc_address =
            config
                .indexer
                .validator_listen_address
                .unwrap_or_else(|| match config.network() {
                    crate::network::Network::Consensus(
                        zcash_protocol::consensus::Network::MainNetwork,
                    ) => "127.0.0.1:8232".parse().unwrap(),
                    _ => "127.0.0.1:18232".parse().unwrap(),
                });

        let db_path = config.indexer.db_path.clone().ok_or(
            FrameworkErrorKind::ComponentError
                .context(ErrorKind::Init.context("indexer.db_path must be set (for now)")),
        )?;

        self.config = Some(FetchServiceConfig::new(
            validator_rpc_address,
            config.indexer.validator_cookie_auth.unwrap_or(false),
            config.indexer.validator_cookie_path.clone(),
            config.indexer.validator_user.clone(),
            config.indexer.validator_password.clone(),
            None,
            None,
            config.indexer.map_capacity,
            config.indexer.map_shard_amount,
            db_path,
            config.indexer.db_size,
            config.network().to_zebra(),
            false,
            // Setting this to `true` causes start-up to block on completely filling the
            // cache. Zaino's DB currently only contains a cache of CompactBlocks, so we
            // make do for now with uncached queries.
            // TODO: https://github.com/zingolabs/zaino/issues/249
            true,
        ));

        Ok(())
    }
}

impl ChainView {
    /// Called automatically after `TokioComponent` is initialized.
    pub fn init_tokio(&mut self, tokio_cmp: &TokioComponent) -> Result<(), FrameworkError> {
        let config = self.config.take().expect("configured");
        let runtime = tokio_cmp.runtime()?;

        info!("Starting Zaino indexer");
        let indexer = self.indexer.clone();
        let started_tx = self.started_tx.take().expect("init_tokio only called once");
        runtime.block_on(async {
            *indexer.write().await = Some(
                IndexerService::spawn(config)
                    .await
                    .map_err(|e| FrameworkErrorKind::ComponentError.context(e))?,
            );
            started_tx.send_modify(|started| *started = true);
            Ok::<_, FrameworkError>(())
        })?;

        let task = runtime.spawn(async move {
            let mut server_interval =
                tokio::time::interval(tokio::time::Duration::from_millis(100));

            loop {
                server_interval.tick().await;

                let service = indexer.read().await;
                let status = match service.as_ref() {
                    Some(service) => service.inner_ref().status(),
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

        self.serve_task = Some(task);

        Ok(())
    }

    pub(crate) async fn subscribe(
        &mut self,
    ) -> Result<IndexerSubscriber<FetchServiceSubscriber>, Error> {
        // Subscribers block on the indexer starting.
        let started = *self.started_rx.borrow_and_update();
        if !started {
            self.started_rx
                .changed()
                .await
                .map_err(|_| ErrorKind::Generic.context("ChainState indexer is not running"))?;
        }

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
