//! A thin, mockable seam over the read operations [`ZebraChainView`](super::ZebraChainView)
//! needs. The real impl issues `ReadStateService` requests and converts results; a test
//! mock substitutes in-memory data so the view's logic is unit-testable without a live
//! zebrad.

use std::future::Future;

use tower::ServiceExt as _;
use zcash_client_backend::data_api::chain::CommitmentTreeRoot;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;
use zebra_chain::subtree::NoteCommitmentSubtreeIndex;
use zebra_state::{HashOrHeight, ReadRequest, ReadResponse, ReadStateService};

use super::super::{BlockLocator, ChainBlock, ChainError};
use super::convert;

/// The header data the height→hash resolve walk needs.
#[derive(Clone, Debug)]
pub(crate) struct HeaderInfo {
    #[allow(dead_code)] // read only by `block_height` (zcashd-import feature)
    pub height: BlockHeight,
    pub previous_block_hash: BlockHash,
}

/// A mined transaction located on the best chain.
pub(crate) struct MinedTxInfo {
    pub raw: Vec<u8>,
    pub height: BlockHeight,
    pub block_time: u32,
}

/// A transaction located on a non-best chain.
pub(crate) struct SideTxInfo {
    pub raw: Vec<u8>,
    pub block_hash: BlockHash,
}

/// Read operations over a chain backend, returning wallet-side types.
pub(crate) trait ChainReader: Clone + Send + Sync + 'static {
    fn tip(&self) -> impl Future<Output = Result<Option<ChainBlock>, ChainError>> + Send;
    fn best_chain_block_hash(
        &self,
        height: BlockHeight,
    ) -> impl Future<Output = Result<Option<BlockHash>, ChainError>> + Send;
    /// Block by hash across any non-finalized chain or the finalized DB (reorg-immune),
    /// as raw canonical bytes.
    fn raw_block_by_hash(
        &self,
        hash: BlockHash,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, ChainError>> + Send;
    fn block_header_by_hash(
        &self,
        hash: BlockHash,
    ) -> impl Future<Output = Result<Option<HeaderInfo>, ChainError>> + Send;
    fn sapling_tree_bytes(
        &self,
        hash: BlockHash,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, ChainError>> + Send;
    fn orchard_tree_bytes(
        &self,
        hash: BlockHash,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, ChainError>> + Send;
    fn find_fork_point(
        &self,
        locator: &BlockLocator,
    ) -> impl Future<Output = Result<Option<ChainBlock>, ChainError>> + Send;
    fn transaction(
        &self,
        txid: zebra_chain::transaction::Hash,
    ) -> impl Future<Output = Result<Option<MinedTxInfo>, ChainError>> + Send;
    fn side_chain_transaction(
        &self,
        txid: zebra_chain::transaction::Hash,
    ) -> impl Future<Output = Result<Option<SideTxInfo>, ChainError>> + Send;
    fn sapling_subtree_roots(
        &self,
    ) -> impl Future<Output = Result<Vec<CommitmentTreeRoot<sapling::Node>>, ChainError>> + Send;
    fn orchard_subtree_roots(
        &self,
    ) -> impl Future<
        Output = Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, ChainError>,
    > + Send;
}

/// [`ChainReader`] backed by a `zebra-state` `ReadStateService`.
#[derive(Clone)]
pub(crate) struct ReadStateChainReader {
    pub(crate) read_state: ReadStateService,
}

impl ReadStateChainReader {
    async fn call(&self, req: ReadRequest) -> Result<ReadResponse, ChainError> {
        self.read_state
            .clone()
            .oneshot(req)
            .await
            .map_err(ChainError::backend)
    }
}

impl ChainReader for ReadStateChainReader {
    async fn tip(&self) -> Result<Option<ChainBlock>, ChainError> {
        match self.call(ReadRequest::Tip).await? {
            ReadResponse::Tip(Some((h, hash))) => Ok(Some(ChainBlock {
                height: convert::height(h),
                hash: convert::block_hash(hash),
            })),
            ReadResponse::Tip(None) => Ok(None),
            other => unreachable!("unexpected response to Tip: {other:?}"),
        }
    }

    async fn best_chain_block_hash(
        &self,
        height: BlockHeight,
    ) -> Result<Option<BlockHash>, ChainError> {
        match self
            .call(ReadRequest::BestChainBlockHash(convert::to_zebra_height(
                height,
            )))
            .await?
        {
            ReadResponse::BlockHash(opt) => Ok(opt.map(convert::block_hash)),
            other => unreachable!("unexpected response to BestChainBlockHash: {other:?}"),
        }
    }

    async fn raw_block_by_hash(&self, hash: BlockHash) -> Result<Option<Vec<u8>>, ChainError> {
        match self
            .call(ReadRequest::AnyChainBlock(HashOrHeight::Hash(
                convert::to_zebra_hash(&hash),
            )))
            .await?
        {
            ReadResponse::Block(Some(block)) => Ok(Some(convert::block_to_bytes(&block)?)),
            ReadResponse::Block(None) => Ok(None),
            other => unreachable!("unexpected response to AnyChainBlock: {other:?}"),
        }
    }

    async fn block_header_by_hash(
        &self,
        hash: BlockHash,
    ) -> Result<Option<HeaderInfo>, ChainError> {
        // `BlockHeader` returns an error (not a `None` response) when the hash is unknown.
        // For an in-process read service the only expected failure is "not found", so map
        // any error to `None`; callers translate that into a reorg/absent signal.
        match self
            .read_state
            .clone()
            .oneshot(ReadRequest::BlockHeader(HashOrHeight::Hash(
                convert::to_zebra_hash(&hash),
            )))
            .await
        {
            Ok(ReadResponse::BlockHeader { header, height, .. }) => Ok(Some(HeaderInfo {
                height: convert::height(height),
                previous_block_hash: convert::block_hash(header.previous_block_hash),
            })),
            Ok(other) => unreachable!("unexpected response to BlockHeader: {other:?}"),
            Err(_) => Ok(None),
        }
    }

    async fn sapling_tree_bytes(&self, hash: BlockHash) -> Result<Option<Vec<u8>>, ChainError> {
        match self
            .call(ReadRequest::SaplingTree(HashOrHeight::Hash(
                convert::to_zebra_hash(&hash),
            )))
            .await?
        {
            ReadResponse::SaplingTree(opt) => Ok(opt.map(|tree| tree.to_rpc_bytes())),
            other => unreachable!("unexpected response to SaplingTree: {other:?}"),
        }
    }

    async fn orchard_tree_bytes(&self, hash: BlockHash) -> Result<Option<Vec<u8>>, ChainError> {
        match self
            .call(ReadRequest::OrchardTree(HashOrHeight::Hash(
                convert::to_zebra_hash(&hash),
            )))
            .await?
        {
            ReadResponse::OrchardTree(opt) => Ok(opt.map(|tree| tree.to_rpc_bytes())),
            other => unreachable!("unexpected response to OrchardTree: {other:?}"),
        }
    }

    async fn find_fork_point(
        &self,
        locator: &BlockLocator,
    ) -> Result<Option<ChainBlock>, ChainError> {
        let known_blocks = locator
            .hashes()
            .iter()
            .map(convert::to_zebra_hash)
            .collect();
        match self
            .call(ReadRequest::FindForkPoint { known_blocks })
            .await?
        {
            ReadResponse::ForkPoint(opt) => Ok(opt.map(|(h, hash)| ChainBlock {
                height: convert::height(h),
                hash: convert::block_hash(hash),
            })),
            other => unreachable!("unexpected response to FindForkPoint: {other:?}"),
        }
    }

    async fn transaction(
        &self,
        txid: zebra_chain::transaction::Hash,
    ) -> Result<Option<MinedTxInfo>, ChainError> {
        match self.call(ReadRequest::Transaction(txid)).await? {
            ReadResponse::Transaction(Some(mined)) => Ok(Some(MinedTxInfo {
                raw: convert::tx_to_bytes(&mined.tx)?,
                height: convert::height(mined.height),
                block_time: mined
                    .block_time
                    .timestamp()
                    .try_into()
                    .map_err(ChainError::invalid_data)?,
            })),
            ReadResponse::Transaction(None) => Ok(None),
            other => unreachable!("unexpected response to Transaction: {other:?}"),
        }
    }

    async fn side_chain_transaction(
        &self,
        txid: zebra_chain::transaction::Hash,
    ) -> Result<Option<SideTxInfo>, ChainError> {
        match self.call(ReadRequest::AnyChainTransaction(txid)).await? {
            ReadResponse::AnyChainTransaction(Some(zebra_state::AnyTx::Side((tx, hash)))) => {
                Ok(Some(SideTxInfo {
                    raw: convert::tx_to_bytes(&tx)?,
                    block_hash: convert::block_hash(hash),
                }))
            }
            // `Mined` is covered by `transaction`; ignore it here.
            ReadResponse::AnyChainTransaction(_) => Ok(None),
            other => unreachable!("unexpected response to AnyChainTransaction: {other:?}"),
        }
    }

    async fn sapling_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<sapling::Node>>, ChainError> {
        match self
            .call(ReadRequest::SaplingSubtrees {
                start_index: NoteCommitmentSubtreeIndex(0),
                limit: None,
            })
            .await?
        {
            ReadResponse::SaplingSubtrees(map) => Ok(map
                .into_values()
                .map(|d| {
                    CommitmentTreeRoot::from_parts(BlockHeight::from_u32(d.end_height.0), d.root)
                })
                .collect()),
            other => unreachable!("unexpected response to SaplingSubtrees: {other:?}"),
        }
    }

    async fn orchard_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, ChainError> {
        match self
            .call(ReadRequest::OrchardSubtrees {
                start_index: NoteCommitmentSubtreeIndex(0),
                limit: None,
            })
            .await?
        {
            ReadResponse::OrchardSubtrees(map) => map
                .into_values()
                .map(|d| {
                    let node = Option::from(orchard::tree::MerkleHashOrchard::from_bytes(
                        &d.root.to_repr(),
                    ))
                    .ok_or_else(|| {
                        ChainError::invalid_data("non-canonical orchard subtree root")
                    })?;
                    Ok(CommitmentTreeRoot::from_parts(
                        BlockHeight::from_u32(d.end_height.0),
                        node,
                    ))
                })
                .collect(),
            other => unreachable!("unexpected response to OrchardSubtrees: {other:?}"),
        }
    }
}
