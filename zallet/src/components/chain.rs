use std::fmt;
use std::ops::Range;

use futures::StreamExt;
use incrementalmerkletree::frontier::CommitmentTree;
use jsonrpsee::tracing::{error, info, warn};
use tokio::net::lookup_host;
use zaino_common::{CacheConfig, DatabaseConfig, StorageConfig};
use zaino_fetch::jsonrpsee::connector::JsonRpSeeConnector;
use zaino_state::{
    ChainIndex as _, ChainIndexConfig, ChainIndexSnapshot, NodeBackedChainIndex,
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
    config::ZalletConfig,
    error::{Error, ErrorKind},
    network::Network,
};

use super::TaskHandle;

/// Converts a `zcash_protocol` block height into a Zaino block height.
fn to_zaino_height(height: BlockHeight) -> zaino_state::Height {
    u32::from(height)
        .try_into()
        .expect("we won't hit max height for a while")
}

/// The Zaino finalised-state database schema version to target.
///
/// `1` selects Zaino's latest v1 finalised-state schema. We run the indexer in ephemeral
/// mode (see [`ChainIndexConfig::new`] below), so no persistent finalised-state database
/// is actually opened and this value has no on-disk effect; it is set to the current
/// schema version for forward-compatibility if ephemeral mode is ever disabled.
const ZAINO_FINALISED_DB_VERSION: u32 = 1;

#[derive(Clone)]
pub(crate) struct Chain {
    subscriber: NodeBackedChainIndexSubscriber,
    /// Used for submitting transactions to the network (`ChainIndex` is a read-only view
    /// of the chain).
    fetcher: JsonRpSeeConnector,
    params: Network,
}

impl fmt::Debug for Chain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chain").finish_non_exhaustive()
    }
}

impl Chain {
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

        let indexer_config = ChainIndexConfig::new(
            StorageConfig {
                cache: CacheConfig::default(),
                database: DatabaseConfig {
                    path: config.indexer_db_path().to_path_buf(),
                    // Unused in ephemeral mode (no persistent finalised-state
                    // database is opened).
                    size: zaino_common::DatabaseSize(0),
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

    /// Broadcasts a transaction to the mempool.
    ///
    /// Returns an error if the transaction failed to be submitted to a single node. No
    /// broadcast guarantees are provided beyond this; transactions should be periodically
    /// rebroadcast while they are unmined and unexpired.
    pub(crate) async fn broadcast_transaction(&self, tx: &Transaction) -> Result<(), Error> {
        let mut tx_bytes = vec![];
        tx.write(&mut tx_bytes)
            .map_err(|e| ErrorKind::Generic.context(e))?;

        self.fetcher
            .send_raw_transaction(hex::encode(&tx_bytes))
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?;

        Ok(())
    }

    /// Returns a stable view of the chain as of the current chain tip.
    ///
    /// The data viewable through the returned [`ChainView`] is guaranteed to be available
    /// as long as (any clone of) the returned instance is live, regardless of what new
    /// blocks or reorgs are observed by the underlying chain indexer.
    pub(crate) async fn snapshot(&self) -> Result<ChainView, Error> {
        let snapshot = self
            .subscriber
            .snapshot_nonfinalized_state()
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?;

        Ok(ChainView {
            chain: self.subscriber.clone(),
            snapshot,
            params: self.params,
        })
    }

    pub(crate) async fn get_sapling_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<sapling::Node>>, Error> {
        self.subscriber
            .get_subtree_roots(ShieldedPool::Sapling, 0, None)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
            .into_iter()
            .map(|(root_hash, end_height)| {
                Ok(CommitmentTreeRoot::from_parts(
                    BlockHeight::from_u32(end_height),
                    sapling::Node::from_bytes(root_hash)
                        .expect("zaino should provide canonical encodings"),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()
    }

    pub(crate) async fn get_orchard_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, Error> {
        self.subscriber
            .get_subtree_roots(ShieldedPool::Orchard, 0, None)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
            .into_iter()
            .map(|(root_hash, end_height)| {
                Ok(CommitmentTreeRoot::from_parts(
                    BlockHeight::from_u32(end_height),
                    orchard::tree::MerkleHashOrchard::from_bytes(&root_hash)
                        .expect("zaino should provide canonical encodings"),
                ))
            })
            .collect::<Result<Vec<_>, Error>>()
    }
}

/// A stable view of the chain as of a particular chain tip.
///
/// The data viewable through an instance of `ChainView` is guaranteed to be available
/// as long as (any clone of) the instance is live, regardless of what new blocks or
/// reorgs are observed by the underlying chain indexer.
#[derive(Clone)]
pub(crate) struct ChainView {
    chain: NodeBackedChainIndexSubscriber,
    snapshot: ChainIndexSnapshot,
    params: Network,
}

impl ChainView {
    /// Returns the current chain tip.
    pub(crate) async fn tip(&self) -> Result<ChainBlock, Error> {
        let best_tip = self
            .chain
            .best_chaintip(&self.snapshot)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?;

        Ok(ChainBlock::from_zaino((best_tip.hash, best_tip.height)))
    }

    /// Returns the height of the given block, if it is in the main chain within this
    /// chain view.
    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    pub(crate) async fn block_height(
        &self,
        hash: &BlockHash,
    ) -> Result<Option<BlockHeight>, Error> {
        Ok(self
            .chain
            .get_block_height(&self.snapshot, zaino_state::BlockHash(hash.0))
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
            .map(|height| BlockHeight::from_u32(height.into())))
    }

    /// Finds the most recent common ancestor of the given block within this chain view.
    ///
    /// Returns the given block itself if it is on the main chain.
    pub(crate) async fn find_fork_point(
        &self,
        other: &BlockHash,
    ) -> Result<Option<ChainBlock>, Error> {
        Ok(self
            .chain
            .find_fork_point(&self.snapshot, &zaino_state::BlockHash(other.0))
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
            .map(ChainBlock::from_zaino))
    }

    /// Returns the final note commitment tree state for each shielded pool, as of the
    /// given block height.
    ///
    /// Returns `None` if the height is greater than the chain tip for this chain view.
    pub(crate) async fn tree_state_as_of(
        &self,
        height: BlockHeight,
    ) -> Result<Option<ChainState>, Error> {
        let block_hash = self
            .chain
            .get_block_hash(&self.snapshot, to_zaino_height(height))
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?;

        let chain_state = if let Some(hash) = block_hash {
            let (sapling_treestate, orchard_treestate) = self
                .chain
                .get_treestate(&hash)
                .await
                .map_err(|e| ErrorKind::Generic.context(e))?;

            let final_sapling_tree = match sapling_treestate {
                None => CommitmentTree::empty(),
                Some(sapling_tree_bytes) => read_commitment_tree::<
                    sapling::Node,
                    _,
                    { sapling::NOTE_COMMITMENT_TREE_DEPTH },
                >(&sapling_tree_bytes[..])
                .map_err(|e| ErrorKind::Generic.context(e))?,
            }
            .to_frontier();

            let final_orchard_tree = match orchard_treestate {
                None => CommitmentTree::empty(),
                Some(orchard_tree_bytes) => read_commitment_tree::<
                    orchard::tree::MerkleHashOrchard,
                    _,
                    { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                >(&orchard_tree_bytes[..])
                .map_err(|e| ErrorKind::Generic.context(e))?,
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

    /// Returns the block header at the given height, or `None` if the height is above
    /// this view's chain tip.
    pub(crate) async fn get_block_header(
        &self,
        height: BlockHeight,
    ) -> Result<Option<BlockHeader>, Error> {
        self.get_block_inner(height, |block_bytes| {
            // Read the header, ignore the transactions.
            BlockHeader::read(block_bytes.as_slice())
                .map_err(|e| ErrorKind::Generic.context(e).into())
        })
        .await
    }

    /// Returns the block at the given height, or `None` if the height is above this
    /// view's chain tip.
    pub(crate) async fn get_block(&self, height: BlockHeight) -> Result<Option<Block>, Error> {
        self.get_block_inner(height, |block_bytes| {
            Block::read(block_bytes.as_slice(), &self.params)
                .map_err(|e| ErrorKind::Generic.context(e).into())
        })
        .await
    }

    async fn get_block_inner<T>(
        &self,
        height: BlockHeight,
        f: impl FnOnce(Vec<u8>) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        let height = to_zaino_height(height);
        // TODO: Should return `impl futures::TryStream` if it is to be fallible.
        if let Some(stream) = self
            .chain
            .get_block_range(&self.snapshot, height, Some(height))
        {
            tokio::pin!(stream);
            let block_bytes = match stream.next().await {
                None => return Ok(None),
                Some(ret) => ret.map_err(|e| ErrorKind::Generic.context(e)),
            }?;

            f(block_bytes).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Produces a contiguous stream of blocks from the given start height to this view's
    /// chain tip, inclusive.
    ///
    /// Returns an empty stream if `start` is greater than this view's chain tip.
    pub(crate) fn stream_blocks_to_tip(
        &self,
        start: BlockHeight,
    ) -> impl futures::Stream<Item = Result<Block, Error>> {
        self.stream_blocks_inner(start, None)
    }

    /// Produces a contiguous stream of blocks over the given range.
    ///
    /// Returns an empty stream if `range` includes block heights greater than this view's
    /// chain tip.
    pub(crate) fn stream_blocks(
        &self,
        range: &Range<BlockHeight>,
    ) -> impl futures::Stream<Item = Result<Block, Error>> {
        self.stream_blocks_inner(range.start, Some(range.end - 1))
    }

    /// Produces a contiguous stream of blocks from `start` to `end` inclusive.
    fn stream_blocks_inner(
        &self,
        start: BlockHeight,
        end: Option<BlockHeight>,
    ) -> impl futures::Stream<Item = Result<Block, Error>> {
        // TODO: Should return `impl futures::TryStream` if it is to be fallible.
        if let Some(stream) = self.chain.get_block_range(
            &self.snapshot,
            to_zaino_height(start),
            end.map(to_zaino_height),
        ) {
            stream
                .map(|res| {
                    res.map_err(|e| ErrorKind::Generic.context(e).into())
                        .and_then(|block_bytes| {
                            Block::read(block_bytes.as_slice(), &self.params)
                                .map_err(|e| ErrorKind::Sync.context(e).into())
                        })
                })
                .boxed()
        } else {
            futures::stream::empty().boxed()
        }
    }

    /// Returns a stream of the current transactions within the mempool.
    ///
    /// The stream ends when the chain tip block hash changes, signalling that either a
    /// new block has been mined or a reorg has occurred.
    ///
    /// Returns `None` if the chain tip has changed since this view was captured.
    pub(crate) async fn get_mempool_stream(
        &self,
    ) -> Result<Option<impl futures::Stream<Item = Transaction>>, Error> {
        let mempool_height = self.tip().await?.height + 1;
        let consensus_branch_id = consensus::BranchId::for_height(&self.params, mempool_height);

        Ok(self
            .chain
            .get_mempool_stream(Some(&self.snapshot))
            .map(move |stream| {
                stream.filter_map(move |result| async move {
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
            }))
    }

    /// Returns the transaction with the given txid, if known.
    pub(crate) async fn get_transaction(&self, txid: TxId) -> Result<Option<ChainTx>, Error> {
        let zaino_txid = zaino_state::TransactionHash::from(*txid.as_ref());

        let (inner, raw) = match self
            .chain
            .get_raw_transaction(&self.snapshot, &zaino_txid)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
        {
            None => return Ok(None),
            Some((raw_tx, branch_id)) => {
                let consensus_branch_id = match branch_id {
                    // If `try_from` fails, it indicates a dependency versioning problem.
                    Some(id) => consensus::BranchId::try_from(id)
                        .map_err(|e| ErrorKind::Generic.context(e))?,
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
                    .map_err(|e| ErrorKind::Generic.context(e))?;

                (tx, raw_tx)
            }
        };

        let (block_hash, mined_height, in_best_chain) = match self
            .chain
            .get_transaction_status(&self.snapshot, &zaino_txid)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
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

    /// Returns the current status of the given transaction.
    pub(crate) async fn get_transaction_status(
        &self,
        txid: TxId,
    ) -> Result<TransactionStatus, Error> {
        let zaino_txid = zaino_state::TransactionHash::from(*txid.as_ref());

        let status = self
            .chain
            .get_transaction_status(&self.snapshot, &zaino_txid)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?;

        Ok(match status {
            (Some(BestChainLocation::Block(_, height)), _) => {
                TransactionStatus::Mined(BlockHeight::from_u32(height.into()))
            }
            (Some(BestChainLocation::Mempool(_)), _) => TransactionStatus::NotInMainChain,
            (None, orphans) if orphans.is_empty() => TransactionStatus::TxidNotRecognized,
            (None, _) => TransactionStatus::NotInMainChain,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChainBlock {
    pub(crate) height: BlockHeight,
    pub(crate) hash: BlockHash,
}

impl ChainBlock {
    fn from_zaino((hash, height): (zaino_state::BlockHash, zaino_state::Height)) -> Self {
        Self {
            height: BlockHeight::from_u32(height.into()),
            hash: BlockHash(hash.0),
        }
    }
}

pub(crate) struct ChainTx {
    pub(crate) inner: Transaction,
    pub(crate) raw: Vec<u8>,
    pub(crate) block_hash: Option<BlockHash>,
    pub(crate) mined_height: Option<BlockHeight>,
    pub(crate) block_time: Option<u32>,
}
