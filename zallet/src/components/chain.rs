#![allow(deprecated)] // For zaino

use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;

use futures::StreamExt;
use incrementalmerkletree::frontier::CommitmentTree;
use jsonrpsee::tracing::info;
use orchard::tree::MerkleHashOrchard;
use tracing::warn;
use zaino_common::{CacheConfig, DatabaseConfig, StorageConfig};
use zaino_fetch::jsonrpsee::connector::JsonRpSeeConnector;
use zaino_state::{
    BlockCacheConfig, ChainIndex, NodeBackedChainIndexSubscriber, NonfinalizedBlockCacheSnapshot,
    StatusType,
    chain_index::{
        NodeBackedChainIndex, NonFinalizedSnapshot,
        source::ValidatorConnector,
        types::{BestChainLocation, NonBestChainLocation},
    },
};
use zcash_client_backend::data_api::{
    TransactionStatus,
    chain::{ChainState, CommitmentTreeRoot},
};
use zcash_encoding::Vector;
use zcash_primitives::{
    block::{BlockHash, BlockHeader},
    merkle_tree::read_commitment_tree,
    transaction::{Transaction, TransactionData},
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

pub(super) type ChainIndexer = NodeBackedChainIndexSubscriber;

#[derive(Clone)]
pub(crate) struct Chain {
    subscriber: NodeBackedChainIndexSubscriber,
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

        info!("Starting Zaino indexer");

        let fetcher = JsonRpSeeConnector::new_from_config_parts(
            validator_rpc_address.into(),
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

        let config = BlockCacheConfig::new(
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
            // TODO: Set this sensibly once Zaino documents how to do so. For now, this is
            // copied from a Zaino `From` impl.
            1,
            params.to_zaino(),
            false,
        );

        let indexer = NodeBackedChainIndex::new(ValidatorConnector::Fetch(fetcher), config)
            .await
            .map_err(|e| ErrorKind::Init.context(e))?;

        let chain = Self {
            subscriber: indexer.subscriber().await,
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

    // TODO: Decide whether we need this, or if we should just rely on the underlying
    // chain indexer to keep track of its state within its own calls.
    async fn subscribe(&self) -> Result<ChainIndexer, Error> {
        match self.subscriber.status() {
            StatusType::Spawning | StatusType::Syncing | StatusType::Ready | StatusType::Busy => {
                Ok(self.subscriber.clone())
            }
            StatusType::Closing
            | StatusType::Offline
            | StatusType::RecoverableError
            | StatusType::CriticalError => Err(ErrorKind::Generic
                .context("ChainState indexer is not running")
                .into()),
        }
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
        // let raw_transaction_hex = hex::encode(&tx_bytes);

        todo!("Broadcast the tx");

        Ok(())
    }

    /// Returns a stable view of the chain as of the current chain tip.
    ///
    /// The data viewable through the returned [`ChainView`] is guaranteed to be available
    /// as long as (any clone of) the returned instance is live, regardless of what new
    /// blocks or reorgs are observed by the underlying chain indexer.
    pub(crate) fn snapshot(&self) -> ChainView {
        ChainView {
            chain: self.subscriber.clone(),
            snapshot: self.subscriber.snapshot_nonfinalized_state(),
            params: self.params.clone(),
        }
    }

    pub(crate) async fn get_sapling_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<sapling::Node>>, Error> {
        // self.subscriber
        //     .z_get_subtrees_by_index("sapling".into(), NoteCommitmentSubtreeIndex(0), None)
        //     .await?
        //     .subtrees()
        //     .iter()
        //     .map(|subtree| {
        //         let mut root_hash = [0; 32];
        //         hex::decode_to_slice(&subtree.root, &mut root_hash).map_err(|e| {
        //             FetchServiceError::RpcError(RpcError::new_from_legacycode(
        //                 LegacyCode::Deserialization,
        //                 format!("Invalid subtree root: {e}"),
        //             ))
        //         })?;
        //         Ok(CommitmentTreeRoot::from_parts(
        //             BlockHeight::from_u32(subtree.end_height.0),
        //             sapling::Node::from_bytes(root_hash).unwrap(),
        //         ))
        //     })
        //     .collect::<Result<Vec<_>, Error>>()
        todo!()
    }

    pub(crate) async fn get_orchard_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<MerkleHashOrchard>>, Error> {
        todo!()
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
    snapshot: Arc<NonfinalizedBlockCacheSnapshot>,
    params: Network,
}

impl ChainView {
    /// Returns the current chain tip.
    pub(crate) fn tip(&self) -> ChainBlock {
        let best_tip = self.snapshot.best_chaintip();
        ChainBlock::from_zaino((best_tip.blockhash, best_tip.height))
    }

    /// Finds the most recent common ancestor of the given block within this chain view.
    ///
    /// Returns the given block itself if it is on the main chain.
    pub(crate) fn find_fork_point(&self, other: &BlockHash) -> Result<Option<ChainBlock>, Error> {
        Ok(self
            .chain
            .find_fork_point(&self.snapshot, &zaino_state::BlockHash(other.0))
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
        let chain_state = if let Some(block) = self.snapshot.get_chainblock_by_height(
            &u32::from(height)
                .try_into()
                .expect("we won't hit max height for a while"),
        ) {
            let (sapling_treestate, orchard_treestate) = self
                .chain
                .get_treestate(block.hash())
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
                    MerkleHashOrchard,
                    _,
                    { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
                >(&orchard_tree_bytes[..])
                .map_err(|e| ErrorKind::Generic.context(e))?,
            }
            .to_frontier();

            Some(ChainState::new(
                height,
                BlockHash(block.hash().0),
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
            Block::parse(block_bytes, height, &self.params)
        })
        .await
    }

    async fn get_block_inner<T>(
        &self,
        height: BlockHeight,
        f: impl FnOnce(Vec<u8>) -> Result<T, Error>,
    ) -> Result<Option<T>, Error> {
        let height = u32::from(height)
            .try_into()
            .expect("we won't hit max height for a while");
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
    ) -> impl futures::Stream<Item = Result<(BlockHeight, Block), Error>> {
        self.stream_blocks_inner(start, None)
    }

    /// Produces a contiguous stream of blocks over the given range.
    ///
    /// Returns an empty stream if `range` includes block heights greater than this view's
    /// chain tip.
    pub(crate) fn stream_blocks(
        &self,
        range: &Range<BlockHeight>,
    ) -> impl futures::Stream<Item = Result<(BlockHeight, Block), Error>> {
        self.stream_blocks_inner(range.start, Some(range.end - 1))
    }

    /// Produces a contiguous stream of blocks from `start` to `end` inclusive.
    fn stream_blocks_inner(
        &self,
        start: BlockHeight,
        end: Option<BlockHeight>,
    ) -> impl futures::Stream<Item = Result<(BlockHeight, Block), Error>> {
        // TODO: Should return `impl futures::TryStream` if it is to be fallible.
        if let Some(stream) = self.chain.get_block_range(
            &self.snapshot,
            u32::from(start)
                .try_into()
                .expect("we won't hit max height for a while"),
            end.map(u32::from)
                .map(|h| h.try_into().expect("we won't hit max height for a while")),
        ) {
            stream
                .zip(futures::stream::iter(u32::from(start)..))
                .map(|(res, height)| {
                    res.map_err(|e| ErrorKind::Generic.context(e).into())
                        .and_then(|block_bytes| {
                            let height = BlockHeight::from_u32(height);
                            Block::parse(block_bytes, height, &self.params)
                                .map(|block| (height, block))
                        })
                })
                .boxed()
        } else {
            futures::stream::empty().boxed()
        }
    }

    /// Returns a stream of the current transactions within the mempool.
    ///
    /// The strean ends when the chain tip block hash changes, signalling that either a
    /// new block has been mined or a reorg has occured.
    ///
    /// Returns `None` if the chain tip has changed since this view was captured.
    pub(crate) fn get_mempool_stream(&self) -> Option<impl futures::Stream<Item = Transaction>> {
        let mempool_height = self.tip().height + 1;
        let consensus_branch_id = consensus::BranchId::for_height(&self.params, mempool_height);

        // TODO: Should return `impl futures::TryStream` if it is to be fallible.
        self.chain
            .get_mempool_stream(&self.snapshot)
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
            })
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
            Some((raw_tx, Some(consensus_branch_id))) => {
                let tx = Transaction::read(
                    raw_tx.as_slice(),
                    consensus::BranchId::try_from(consensus_branch_id)
                        // If this fails, it indicates a dependency versioning problem.
                        .map_err(|e| ErrorKind::Generic.context(e))?,
                )
                .map_err(|e| ErrorKind::Generic.context(e))?;

                Ok((tx, raw_tx))
            }
            Some((raw_tx, None)) => {
                // Use the invariant that a transaction can't be mined across a network
                // upgrade boundary, so the expiry height must be in the same epoch as the
                // transaction's target height.
                let tx_data = Transaction::read(raw_tx.as_slice(), consensus::BranchId::Sprout)
                    .map_err(|e| ErrorKind::Generic.context(e))?
                    .into_data();

                let expiry_height = tx_data.expiry_height();
                if expiry_height > BlockHeight::from(0) {
                    let tx = TransactionData::from_parts(
                        tx_data.version(),
                        consensus::BranchId::for_height(&self.params, expiry_height),
                        tx_data.lock_time(),
                        expiry_height,
                        tx_data.transparent_bundle().cloned(),
                        tx_data.sprout_bundle().cloned(),
                        tx_data.sapling_bundle().cloned(),
                        tx_data.orchard_bundle().cloned(),
                    )
                    .freeze()
                    .map_err(|e| ErrorKind::Generic.context(e))?;

                    Ok((tx, raw_tx))
                } else {
                    Err(ErrorKind::Generic
                        .context(format!("Consensus branch ID not known for {}", txid)))
                }
            }
        }?;

        let (block_hash, mined_height) = match self
            .chain
            .get_transaction_status(&self.snapshot, &zaino_txid)
            .await
            .map_err(|e| ErrorKind::Generic.context(e))?
        {
            (Some(BestChainLocation::Block(hash, height)), _) => (
                Some(BlockHash(hash.0)),
                Some(BlockHeight::from_u32(height.into())),
            ),
            (Some(BestChainLocation::Mempool(_)), _) => (None, None),
            (None, orphans) => match orphans.into_iter().next() {
                Some(NonBestChainLocation::Block(hash, height)) => (
                    Some(BlockHash(hash.0)),
                    Some(BlockHeight::from_u32(height.into())),
                ),
                Some(NonBestChainLocation::Mempool(_)) | None => (None, None),
            },
        };

        let block_time = match mined_height {
            None => None,
            Some(height) => self
                .get_block_header(height)
                .await?
                .map(|header| header.time),
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
            (None, orphans) if orphans.is_empty() => TransactionStatus::NotInMainChain,
            (None, _) => TransactionStatus::TxidNotRecognized,
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

pub(crate) struct Block {
    pub(crate) header: BlockHeader,
    pub(crate) vtx: Vec<Transaction>,
}

impl Block {
    fn parse(block_bytes: Vec<u8>, height: BlockHeight, params: &Network) -> Result<Self, Error> {
        let consensus_branch_id = consensus::BranchId::for_height(params, height);
        let mut reader = io::Cursor::new(block_bytes);
        let header = BlockHeader::read(&mut reader).map_err(|e| ErrorKind::Generic.context(e))?;
        let vtx = Vector::read(&mut reader, |r| Transaction::read(r, consensus_branch_id))
            .map_err(|e| ErrorKind::Generic.context(e))?;
        Ok(Self { header, vtx })
    }
}

pub(crate) struct ChainTx {
    pub(crate) inner: Transaction,
    pub(crate) raw: Vec<u8>,
    pub(crate) block_hash: Option<BlockHash>,
    pub(crate) mined_height: Option<BlockHeight>,
    pub(crate) block_time: Option<u32>,
}
