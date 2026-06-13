#![allow(deprecated)] // For zaino

use std::fmt;
use std::sync::Arc;

use jsonrpsee::tracing::{error, info};
use tokio::net::lookup_host;
use tokio::sync::RwLock;
use zaino_common::{CacheConfig, DatabaseConfig, ServiceConfig, StorageConfig};
use zaino_state::{
    FetchService, FetchServiceConfig, FetchServiceSubscriber, IndexerService, IndexerSubscriber,
    Status, StatusType, ZcashService,
};

use crate::{
    config::ZalletConfig,
    error::{Error, ErrorKind},
    fl,
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
                        Err(ErrorKind::Init
                            .context(fl!("err-init-validator-no-addresses", addr = addr_str)))
                    }
                },
                Err(e) => {
                    error!("Failed to resolve validator_address '{}': {}", addr_str, e);
                    Err(ErrorKind::Init.context(fl!(
                        "err-init-validator-resolve-failed",
                        addr = addr_str,
                        error = e.to_string()
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
                        Err(ErrorKind::Init.context(fl!(
                            "err-init-validator-parse-default-failed",
                            addr = default_addr_str,
                            error = e.to_string()
                        )))
                    }
                }
            }
        }?;

        let config = FetchServiceConfig::new(
            resolved_validator_address.to_string(),
            config.indexer.validator_cookie_path.clone(),
            config.indexer.validator_user.clone(),
            config.indexer.validator_password.clone(),
            ServiceConfig::default(),
            StorageConfig {
                cache: CacheConfig::default(),
                database: DatabaseConfig {
                    path: config.indexer_db_path().to_path_buf(),
                    // Unused in ephemeral mode (no persistent finalised-state
                    // database is opened).
                    size: zaino_common::DatabaseSize(0),
                },
            },
            // Run the finalised state ephemerally: no persistent database,
            // finalised reads are served from the backing validator. This
            // replaces the previous `DatabaseSize(0)` workaround for
            // https://github.com/zingolabs/zaino/issues/249, which made the
            // LMDB map fill up and permanently stall the sync loop.
            true,
            config.consensus.network().to_zaino(),
            None,
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
            .ok_or_else(|| ErrorKind::Generic.context(fl!("err-chain-indexer-not-running")))?
            .inner_ref()
            .get_subscriber())
    }
}
