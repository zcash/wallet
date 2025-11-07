use std::fmt;
use std::sync::Arc;

use jsonrpsee::tracing::{error, info};
use tokio::net::lookup_host;
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
        let resolved_validator_address = match config.indexer.validator_address.as_deref() {
            Some(addr_str) => match lookup_host(addr_str).await {
                Ok(mut addrs) => match addrs.next() {
                    Some(socket_addr) => {
                        info!(
                            "Resolved validator_address '{}' to {}",
                            addr_str, socket_addr
                        );
                        Ok(socket_addr)
                    }
                    None => {
                        error!(
                            "validator_address '{}' resolved to no IP addresses",
                            addr_str
                        );
                        Err(ErrorKind::Init.context(format!(
                            "validator_address '{addr_str}' resolved to no IP addresses"
                        )))
                    }
                },
                Err(e) => {
                    error!("Failed to resolve validator_address '{}': {}", addr_str, e);
                    Err(ErrorKind::Init.context(format!(
                        "Failed to resolve validator_address '{addr_str}': {e}"
                    )))
                }
            },
            None => {
                // Default to localhost and standard port based on network
                let default_port = match config.consensus.network() {
                    crate::network::Network::Consensus(
                        zcash_protocol::consensus::Network::MainNetwork,
                    ) => 8232, // Mainnet default RPC port for Zebra/zcashd
                    _ => 18232, // Testnet/Regtest default RPC port for Zebra/zcashd
                };
                let default_addr_str = format!("127.0.0.1:{default_port}");
                info!(
                    "validator_address not set, defaulting to {}",
                    default_addr_str
                );
                match default_addr_str.parse::<std::net::SocketAddr>() {
                    Ok(socket_addr) => Ok(socket_addr),
                    Err(e) => {
                        // This should ideally not happen with a hardcoded IP and port
                        error!(
                            "Failed to parse default validator_address '{}': {}",
                            default_addr_str, e
                        );
                        Err(ErrorKind::Init.context(format!(
                            "Failed to parse default validator_address '{default_addr_str}': {e}"
                        )))
                    }
                }
            }
        }?;

        let config = FetchServiceConfig::new(
            resolved_validator_address,
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
