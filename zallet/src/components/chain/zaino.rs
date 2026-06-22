//! The Zaino-backed implementation of [`Chain`] and [`ChainView`].

use std::fmt;
use std::ops::Range;

use futures::{StreamExt, stream::BoxStream};
use incrementalmerkletree::frontier::CommitmentTree;
use jsonrpsee::tracing::{error, info, warn};
use tokio::net::lookup_host;
use zaino_common::{CacheConfig, DatabaseConfig, StorageConfig};
use zaino_fetch::jsonrpsee::connector::JsonRpSeeConnector;
use zaino_state::{
    BlockCacheConfig, ChainIndex as _, ChainIndexSnapshot, NodeBackedChainIndex,
    NodeBackedChainIndexSubscriber, StatusType,
    chain_index::{
        ShieldedPool,
        source::ValidatorConnector,
        types::{BestChainLocation, NonBestChainLocation},
    },
};
use zcash_client_backend::data_api::{
    TransactionStatus,
    chain::{ChainState, CommitmentTreeRoot},
};
use zcash_primitives::{
    block::{Block, BlockHash, BlockHeader},
    merkle_tree::read_commitment_tree,
    transaction::Transaction,
};
use zcash_protocol::{
    TxId,
    consensus::{self, BlockHeight},
};

use crate::{
    components::TaskHandle,
    config::ZalletConfig,
    error::{Error, ErrorKind},
    network::Network,
};

use super::{Chain, ChainBlock, ChainError, ChainTx, ChainView};

/// Converts a `zcash_protocol` block height into a Zaino block height.
fn to_zaino_height(height: BlockHeight) -> zaino_state::Height {
    u32::from(height)
        .try_into()
        .expect("we won't hit max height for a while")
}

/// The Zaino finalised-state database schema version to target.
///
/// `1` selects Zaino's latest v1 finalised-state schema. We run the indexer in ephemeral
/// mode (see [`BlockCacheConfig::new`] below), so no persistent finalised-state database
/// is actually opened and this value has no on-disk effect; it is set to the current
/// schema version for forward-compatibility if ephemeral mode is ever disabled.
const ZAINO_FINALISED_DB_VERSION: u32 = 1;

#[derive(Clone)]
pub(crate) struct ZainoChain {
    subscriber: NodeBackedChainIndexSubscriber,
    /// Used for submitting transactions to the network (`ChainIndex` is a read-only view
    /// of the chain).
    fetcher: JsonRpSeeConnector,
    params: Network,
}

impl fmt::Debug for ZainoChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZainoChain").finish_non_exhaustive()
    }
}

impl ZainoChain {
    pub(crate) async fn new(config: &ZalletConfig) -> Result<(Self, TaskHandle), Error> {
        let params = config.consensus.network();

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
                let default_port = match params {
                    Network::Consensus(consensus::Network::MainNetwork) => 8232, // Mainnet default RPC port for Zebra/zcashd
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

        let fetcher = JsonRpSeeConnector::new_from_config_parts(
            &resolved_validator_address.to_string(),
            config.indexer.validator_user.clone().unwrap_or_default(),
            config
                .indexer
                .validator_password
                .clone()
                .unwrap_or_default(),
            config.indexer.validator_cookie_path.clone(),
        )
        .await
        .map_err(|e| ErrorKind::Init.context(e))?;

        let indexer_config = BlockCacheConfig::new(
            StorageConfig {
                cache: CacheConfig::default(),
                database: DatabaseConfig {
                    path: config.indexer_db_path().to_path_buf(),
                    // Unused in ephemeral mode (no persistent finalised-state
                    // database is opened).
                    size: zaino_common::DatabaseSize(0),
                    // Unused in ephemeral mode (the finalised-state bulk-sync
                    // write path is never exercised); inherit Zaino's default.
                    ..Default::default()
                },
            },
            ZAINO_FINALISED_DB_VERSION,
            params.to_zaino(),
            // Run the finalised state ephemerally: no persistent database, finalised
            // reads are served from the backing validator.
            true,
        );

        info!("Starting Zaino indexer");
        let indexer =
            NodeBackedChainIndex::new(ValidatorConnector::Fetch(fetcher.clone()), indexer_config)
                .await
                .map_err(|e| ErrorKind::Init.context(e))?;
        info!("Started Zaino indexer");

        let chain = Self {
            subscriber: indexer.subscriber(),
            fetcher,
            params,
        };

        // Spawn a task that stops the indexer when appropriate internal signals occur.
        let task = crate::spawn!("Indexer shutdown", async move {
            let mut server_interval =
                tokio::time::interval(tokio::time::Duration::from_millis(100));

            loop {
                server_interval.tick().await;

                match indexer.status() {
                    // Check for errors.
                    StatusType::Offline | StatusType::CriticalError => {
                        indexer
                            .shutdown()
                            .await
                            .map_err(|e| ErrorKind::Generic.context(e))?;
                        return Err(ErrorKind::Generic.into());
                    }

                    // Check for shutdown signals.
                    StatusType::Closing => return Ok(()),

                    _ => (),
                }
            }
        });

        Ok((chain, task))
    }
}

impl Chain for ZainoChain {
    type View = ZainoChainView;

    async fn broadcast_transaction(&self, tx: &Transaction) -> Result<(), ChainError> {
        let mut tx_bytes = vec![];
        tx.write(&mut tx_bytes).map_err(ChainError::backend)?;

        self.fetcher
            .send_raw_transaction(hex::encode(&tx_bytes))
            .await
            .map_err(ChainError::backend)?;

        Ok(())
    }

    async fn get_sapling_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<sapling::Node>>, ChainError> {
        self.subscriber
            .get_subtree_roots(ShieldedPool::Sapling, 0, None)
            .await
            .map_err(ChainError::backend)?
            .into_iter()
            .map(|(root_hash, end_height)| {
                Ok(CommitmentTreeRoot::from_parts(
                    BlockHeight::from_u32(end_height),
                    sapling::Node::from_bytes(root_hash)
                        .expect("zaino should provide canonical encodings"),
                ))
            })
            .collect::<Result<Vec<_>, ChainError>>()
    }

    async fn get_orchard_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, ChainError> {
        self.subscriber
            .get_subtree_roots(ShieldedPool::Orchard, 0, None)
            .await
            .map_err(ChainError::backend)?
            .into_iter()
            .map(|(root_hash, end_height)| {
                Ok(CommitmentTreeRoot::from_parts(
                    BlockHeight::from_u32(end_height),
                    orchard::tree::MerkleHashOrchard::from_bytes(&root_hash)
                        .expect("zaino should provide canonical encodings"),
                ))
            })
            .collect::<Result<Vec<_>, ChainError>>()
    }

    async fn snapshot(&self) -> Result<ZainoChainView, ChainError> {
        let snapshot = self
            .subscriber
            .snapshot_nonfinalized_state()
            .await
            .map_err(ChainError::backend)?;

        Ok(ZainoChainView {
            chain: self.subscriber.clone(),
            snapshot,
            params: self.params,
        })
    }
}

/// A stable view of the chain as of a particular chain tip.
///
/// The data viewable through an instance of `ZainoChainView` is guaranteed to be available
/// as long as (any clone of) the instance is live, regardless of what new blocks or
/// reorgs are observed by the underlying chain indexer.
#[derive(Clone)]
pub(crate) struct ZainoChainView {
    chain: NodeBackedChainIndexSubscriber,
    snapshot: ChainIndexSnapshot,
    params: Network,
}

impl ChainView for ZainoChainView {
    async fn tip(&self) -> Result<ChainBlock, ChainError> {
        let best_tip = self
            .chain
            .best_chaintip(&self.snapshot)
            .await
            .map_err(ChainError::backend)?;

        Ok(ChainBlock::from_zaino((best_tip.hash, best_tip.height)))
    }

    async fn find_fork_point(
        &self,
        known_tip: &BlockHash,
    ) -> Result<Option<ChainBlock>, ChainError> {
        Ok(self
            .chain
            .find_fork_point(&self.snapshot, &zaino_state::BlockHash(known_tip.0))
            .await
            .map_err(ChainError::backend)?
            .map(ChainBlock::from_zaino))
    }

    async fn tree_state_as_of(
        &self,
        height: BlockHeight,
    ) -> Result<Option<ChainState>, ChainError> {
        let block_hash = self
            .chain
            .get_block_hash(&self.snapshot, to_zaino_height(height))
            .await
            .map_err(ChainError::backend)?;

        let chain_state = if let Some(hash) = block_hash {
            let (sapling_treestate, orchard_treestate) = self
                .chain
                .get_treestate(&hash)
                .await
                .map_err(ChainError::backend)?;

            let final_sapling_tree = match sapling_treestate {
                None => CommitmentTree::empty(),
                Some(sapling_tree_bytes) => read_commitment_tree::<
                    sapling::Node,
                    _,
                    { sapling::NOTE_COMMITMENT_TREE_DEPTH },
                >(&sapling_tree_bytes[..])
                .map_err(ChainError::backend)?,
            }
            .to_frontier();

            let final_orchard_tree = match orchard_treestate {
                None => CommitmentTree::empty(),
                Some(orchard_tree_bytes) => read_commitment_tree::<
                    orchard::tree::MerkleHashOrchard,
                    _,
                    { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                >(&orchard_tree_bytes[..])
                .map_err(ChainError::backend)?,
            }
            .to_frontier();

            Some(ChainState::new(
                height,
                BlockHash(hash.0),
                final_sapling_tree,
                final_orchard_tree,
            ))
        } else {
            None
        };

        Ok(chain_state)
    }

    async fn get_block_header(
        &self,
        height: BlockHeight,
    ) -> Result<Option<BlockHeader>, ChainError> {
        self.get_block_inner(height, |block_bytes| {
            // Read the header, ignore the transactions.
            BlockHeader::read(block_bytes.as_slice()).map_err(ChainError::backend)
        })
        .await
    }

    async fn get_block(&self, height: BlockHeight) -> Result<Option<Block>, ChainError> {
        self.get_block_inner(height, |block_bytes| {
            Block::read(block_bytes.as_slice(), &self.params).map_err(ChainError::backend)
        })
        .await
    }

    fn stream_blocks_to_tip(&self, start: BlockHeight) -> BoxStream<'_, Result<Block, ChainError>> {
        self.stream_blocks_inner(start, None)
    }

    fn stream_blocks(
        &self,
        range: &Range<BlockHeight>,
    ) -> BoxStream<'_, Result<Block, ChainError>> {
        self.stream_blocks_inner(range.start, Some(range.end - 1))
    }

    async fn get_mempool_stream(&self) -> Result<Option<BoxStream<'_, Transaction>>, ChainError> {
        let mempool_height = self.tip().await?.height + 1;
        let consensus_branch_id = consensus::BranchId::for_height(&self.params, mempool_height);

        Ok(self
            .chain
            .get_mempool_stream(Some(&self.snapshot))
            .map(move |stream| {
                stream
                    .filter_map(move |result| async move {
                        result
                            .inspect_err(|e| warn!("Error receiving transaction: {e}"))
                            .ok()
                            .and_then(|raw_tx| {
                                Transaction::read(raw_tx.as_slice(), consensus_branch_id)
                                    .inspect_err(|e| {
                                        warn!("Received invalid transaction from mempool: {e}");
                                    })
                                    .ok()
                            })
                    })
                    .boxed()
            }))
    }

    async fn get_transaction(&self, txid: TxId) -> Result<Option<ChainTx>, ChainError> {
        let zaino_txid = zaino_state::TransactionHash::from(*txid.as_ref());

        let (inner, raw) = match self
            .chain
            .get_raw_transaction(&self.snapshot, &zaino_txid)
            .await
            .map_err(ChainError::backend)?
        {
            None => return Ok(None),
            Some((raw_tx, branch_id)) => {
                let consensus_branch_id = match branch_id {
                    // If `try_from` fails, it indicates a dependency versioning problem.
                    Some(id) => consensus::BranchId::try_from(id).map_err(ChainError::backend)?,
                    // Zaino could not determine the consensus branch ID. This happens for
                    // mempool transactions (when the snapshot's mempool height is unknown)
                    // and for pre-Overwinter transactions (which predate consensus branch
                    // IDs). A transaction cannot be mined across a network upgrade
                    // boundary, and an unmined transaction must be minable at the current
                    // chain tip, so the branch ID at the mempool height (tip + 1) is the
                    // correct parsing target. This matches the fallback used by
                    // `get_mempool_stream`.
                    None => {
                        let mempool_height = self.tip().await?.height + 1;
                        consensus::BranchId::for_height(&self.params, mempool_height)
                    }
                };

                let tx = Transaction::read(raw_tx.as_slice(), consensus_branch_id)
                    .map_err(ChainError::backend)?;

                (tx, raw_tx)
            }
        };

        let (block_hash, mined_height, in_best_chain) = match self
            .chain
            .get_transaction_status(&self.snapshot, &zaino_txid)
            .await
            .map_err(ChainError::backend)?
        {
            (Some(BestChainLocation::Block(hash, height)), _) => (
                Some(BlockHash(hash.0)),
                Some(BlockHeight::from_u32(height.into())),
                true,
            ),
            (Some(BestChainLocation::Mempool(_)), _) => (None, None, false),
            (None, orphans) => match orphans.into_iter().next() {
                Some(NonBestChainLocation::Block(hash, height)) => (
                    Some(BlockHash(hash.0)),
                    Some(BlockHeight::from_u32(height.into())),
                    false,
                ),
                Some(NonBestChainLocation::Mempool(_)) | None => (None, None, false),
            },
        };

        // Only populate the block time for transactions mined into the main chain. The
        // time of an orphaned block is misleading (the transaction is not in the main
        // chain) and can differ from the canonical block at the same height.
        let block_time = match (in_best_chain, mined_height) {
            (true, Some(height)) => self
                .get_block_header(height)
                .await?
                .map(|header| header.time),
            _ => None,
        };

        Ok(Some(ChainTx {
            inner,
            raw,
            block_hash,
            mined_height,
            block_time,
        }))
    }

    async fn get_transaction_status(&self, txid: TxId) -> Result<TransactionStatus, ChainError> {
        let zaino_txid = zaino_state::TransactionHash::from(*txid.as_ref());

        let status = self
            .chain
            .get_transaction_status(&self.snapshot, &zaino_txid)
            .await
            .map_err(ChainError::backend)?;

        Ok(match status {
            (Some(BestChainLocation::Block(_, height)), _) => {
                TransactionStatus::Mined(BlockHeight::from_u32(height.into()))
            }
            (Some(BestChainLocation::Mempool(_)), _) => TransactionStatus::NotInMainChain,
            (None, orphans) if orphans.is_empty() => TransactionStatus::TxidNotRecognized,
            (None, _) => TransactionStatus::NotInMainChain,
        })
    }

    /// Returns the height of the given block, if it is in the main chain within this
    /// chain view.
    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    async fn block_height(&self, hash: &BlockHash) -> Result<Option<BlockHeight>, ChainError> {
        Ok(self
            .chain
            .get_block_height(&self.snapshot, zaino_state::BlockHash(hash.0))
            .await
            .map_err(ChainError::backend)?
            .map(|height| BlockHeight::from_u32(height.into())))
    }
}

impl ZainoChainView {
    async fn get_block_inner<T>(
        &self,
        height: BlockHeight,
        f: impl FnOnce(Vec<u8>) -> Result<T, ChainError>,
    ) -> Result<Option<T>, ChainError> {
        let height = to_zaino_height(height);
        // TODO: Should return `impl futures::TryStream` if it is to be fallible.
        if let Some(stream) = self
            .chain
            .get_block_range(&self.snapshot, height, Some(height))
        {
            tokio::pin!(stream);
            let block_bytes = match stream.next().await {
                None => return Ok(None),
                Some(ret) => ret.map_err(ChainError::backend),
            }?;

            f(block_bytes).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Produces a contiguous stream of blocks from `start` to `end` inclusive.
    fn stream_blocks_inner(
        &self,
        start: BlockHeight,
        end: Option<BlockHeight>,
    ) -> BoxStream<'_, Result<Block, ChainError>> {
        // TODO: Should return `impl futures::TryStream` if it is to be fallible.
        if let Some(stream) = self.chain.get_block_range(
            &self.snapshot,
            to_zaino_height(start),
            end.map(to_zaino_height),
        ) {
            stream
                .map(|res| {
                    res.map_err(ChainError::backend).and_then(|block_bytes| {
                        Block::read(block_bytes.as_slice(), &self.params)
                            .map_err(ChainError::invalid_data)
                    })
                })
                .boxed()
        } else {
            futures::stream::empty().boxed()
        }
    }
}

impl ChainBlock {
    fn from_zaino((hash, height): (zaino_state::BlockHash, zaino_state::Height)) -> Self {
        Self {
            height: BlockHeight::from_u32(height.into()),
            hash: BlockHash(hash.0),
        }
    }
}
