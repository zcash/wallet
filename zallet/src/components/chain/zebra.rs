//! The zebra-state + Zebra-RPC backed implementation of [`Chain`] and [`ChainView`].
//!
//! Reads finalized chain data directly from a local zebrad's state database (opened
//! read-only as a RocksDB secondary), follows the non-finalized tip over zebrad's gRPC
//! indexer interface, and uses a small direct JSON-RPC client for mempool access and
//! transaction submission.

use std::ops::Range;
use std::sync::Arc;

use futures::stream::BoxStream;
use jsonrpsee::tracing::info;
use tokio::net::lookup_host;
use zcash_client_backend::data_api::{
    TransactionStatus,
    chain::{ChainState, CommitmentTreeRoot},
};
use zcash_primitives::{
    block::{Block, BlockHash, BlockHeader},
    transaction::Transaction,
};
use zcash_protocol::{TxId, consensus::BlockHeight};
use zebra_rpc::sync::init_read_state_with_syncer;
use zebra_state::{ChainTipChange, LatestChainTip, ReadStateService};

use super::{Chain, ChainBlock, ChainError, ChainTx, ChainView};
use crate::{
    commands::resolve_datadir_path,
    components::TaskHandle,
    config::ZalletConfig,
    error::{Error, ErrorKind},
    network::Network,
};

mod rpc;
use rpc::ValidatorRpcClient;

/// Aborts the wrapped syncer task when the last clone of the owning [`Arc`] is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A handle to chain data read from a local zebrad's `zebra-state`.
#[derive(Clone)]
pub(crate) struct ZebraChain {
    read_state_service: ReadStateService,
    latest_tip: LatestChainTip,
    tip_change: ChainTipChange,
    validator_rpc: ValidatorRpcClient,
    params: Network,
    _syncer: Arc<AbortOnDrop>,
}

impl std::fmt::Debug for ZebraChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZebraChain").finish_non_exhaustive()
    }
}

impl ZebraChain {
    pub(crate) async fn new(config: &ZalletConfig) -> Result<(Self, TaskHandle), Error> {
        let params = config.consensus.network();

        let rss = config.indexer.read_state_service.as_ref().ok_or_else(|| {
            ErrorKind::Init.context(
                "the zebra-state backend requires an [indexer.read_state_service] config section",
            )
        })?;

        // Validator JSON-RPC client (mempool reads + transaction submission), using the
        // existing [indexer] validator connection settings.
        let validator_address = config
            .indexer
            .validator_address
            .as_deref()
            .ok_or_else(|| ErrorKind::Init.context("indexer.validator_address is required"))?;
        let validator_rpc = ValidatorRpcClient::new(
            validator_address,
            config.indexer.validator_user.as_deref().unwrap_or_default(),
            config
                .indexer
                .validator_password
                .as_deref()
                .unwrap_or_default(),
            config.indexer.validator_cookie_path.as_deref(),
        )?;

        // Resolve the gRPC indexer address used by the non-finalized syncer.
        let grpc_addr = lookup_host(&rss.grpc_address)
            .await
            .map_err(|e| {
                ErrorKind::Init.context(format!(
                    "failed to resolve indexer.read_state_service.grpc_address '{}': {e}",
                    rss.grpc_address,
                ))
            })?
            .next()
            .ok_or_else(|| {
                ErrorKind::Init.context(format!(
                    "indexer.read_state_service.grpc_address '{}' resolved to no IP addresses",
                    rss.grpc_address,
                ))
            })?;

        let zebra_network = params.to_zebra().map_err(|e| ErrorKind::Init.context(e))?;
        let zebra_state_path = resolve_datadir_path(config.datadir(), &rss.zebra_state_path);
        let zebra_config = zebra_state::Config {
            cache_dir: zebra_state_path,
            // The standalone read state service cannot use ephemeral state; it reads
            // zebrad's on-disk database in place.
            ephemeral: false,
            // We are a read-only secondary; never delete or back up zebrad's database.
            delete_old_database: false,
            should_backup_non_finalized_state: false,
            ..Default::default()
        };

        // Fail fast with an actionable error if there is no compatible zebra-state
        // database at the configured path, rather than letting zebra-state silently
        // create a new (empty) database there.
        match zebra_state::state_database_format_version_on_disk(&zebra_config, &zebra_network)
            .map_err(|e| {
                ErrorKind::Init.context(format!(
                    "failed to read the zebra-state database version at '{}': {e}",
                    zebra_config.cache_dir.display(),
                ))
            })? {
            Some(_) => {}
            None => {
                return Err(ErrorKind::Init
                    .context(format!(
                        "no zebra-state v{} database found under '{}'; check that \
                         indexer.read_state_service.zebra_state_path points at zebrad's \
                         state cache directory, and that zebrad's on-disk state format \
                         matches Zallet's zebra-state version",
                        zebra_state::state_database_format_version_in_code().major,
                        zebra_config.cache_dir.display(),
                    ))
                    .into());
            }
        }

        info!("Initializing read-only Zebra state service");
        let (read_state_service, latest_tip, tip_change, sync_task) =
            init_read_state_with_syncer(zebra_config, &zebra_network, grpc_addr)
                .await
                // Outer JoinError from the spawned init task.
                .map_err(|e| ErrorKind::Init.context(e))?
                // Inner BoxError from read-state initialization.
                .map_err(|e| ErrorKind::Init.context(e))?;

        let chain = Self {
            read_state_service,
            latest_tip,
            tip_change,
            validator_rpc,
            params,
            _syncer: Arc::new(AbortOnDrop(sync_task)),
        };

        // Lifecycle task. The syncer is owned by `_syncer` (aborted when the last
        // `ZebraChain` clone drops). This task exists to match the backend lifecycle
        // shape; it runs until aborted on shutdown.
        // TODO: signal syncer failure through this task once the syncer exposes it.
        let task = crate::spawn!("Zebra read-state syncer", async move {
            std::future::pending::<()>().await;
            Ok::<(), Error>(())
        });

        Ok((chain, task))
    }
}

impl Chain for ZebraChain {
    type View = ZebraChainView;

    async fn broadcast_transaction(&self, tx: &Transaction) -> Result<(), ChainError> {
        let mut tx_bytes = vec![];
        tx.write(&mut tx_bytes).map_err(ChainError::backend)?;
        self.validator_rpc
            .send_raw_transaction(hex::encode(&tx_bytes))
            .await
            .map_err(ChainError::backend)?;
        Ok(())
    }

    async fn get_sapling_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<sapling::Node>>, ChainError> {
        todo!("Plan 3: SaplingSubtrees read request")
    }

    async fn get_orchard_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, ChainError> {
        todo!("Plan 3: OrchardSubtrees read request")
    }

    async fn snapshot(&self) -> Result<ZebraChainView, ChainError> {
        // Plan 3 captures the tip and builds the height->hash cache here.
        Ok(ZebraChainView {
            read_state_service: self.read_state_service.clone(),
            latest_tip: self.latest_tip.clone(),
            tip_change: self.tip_change.clone(),
            validator_rpc: self.validator_rpc.clone(),
            params: self.params,
        })
    }
}

/// A pinned view of the chain as of a captured tip.
#[derive(Clone)]
pub(crate) struct ZebraChainView {
    #[allow(dead_code)]
    read_state_service: ReadStateService,
    #[allow(dead_code)]
    latest_tip: LatestChainTip,
    #[allow(dead_code)]
    tip_change: ChainTipChange,
    #[allow(dead_code)]
    validator_rpc: ValidatorRpcClient,
    #[allow(dead_code)]
    params: Network,
}

impl ChainView for ZebraChainView {
    async fn tip(&self) -> Result<ChainBlock, ChainError> {
        todo!("Plan 3: capture tip from latest_tip")
    }

    async fn find_fork_point(
        &self,
        _locator: &[BlockHash],
    ) -> Result<Option<ChainBlock>, ChainError> {
        todo!("Plan 3: FindForkPoint read request")
    }

    async fn tree_state_as_of(
        &self,
        _height: BlockHeight,
    ) -> Result<Option<ChainState>, ChainError> {
        todo!("Plan 3: SaplingTree/OrchardTree by pinned hash")
    }

    async fn get_block_header(
        &self,
        _height: BlockHeight,
    ) -> Result<Option<BlockHeader>, ChainError> {
        todo!("Plan 3: BlockHeader by pinned hash")
    }

    async fn get_block(&self, _height: BlockHeight) -> Result<Option<Block>, ChainError> {
        todo!("Plan 3: AnyChainBlock by pinned hash")
    }

    fn stream_blocks_to_tip(
        &self,
        _start: BlockHeight,
    ) -> BoxStream<'_, Result<Block, ChainError>> {
        todo!("Plan 3: block stream")
    }

    fn stream_blocks(
        &self,
        _range: &Range<BlockHeight>,
    ) -> BoxStream<'_, Result<Block, ChainError>> {
        todo!("Plan 3: block stream")
    }

    async fn get_mempool_stream(&self) -> Result<Option<BoxStream<'_, Transaction>>, ChainError> {
        todo!("Plan 4: JSON-RPC mempool stream ended by ChainTipChange")
    }

    async fn get_transaction(&self, _txid: TxId) -> Result<Option<ChainTx>, ChainError> {
        todo!("Plan 3: Transaction/AnyChainTransaction + mempool fallback")
    }

    async fn get_transaction_status(&self, _txid: TxId) -> Result<TransactionStatus, ChainError> {
        todo!("Plan 3: transaction status")
    }

    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    async fn block_height(&self, _hash: &BlockHash) -> Result<Option<BlockHeight>, ChainError> {
        todo!("Plan 3: BlockHeader(hash).height")
    }
}
