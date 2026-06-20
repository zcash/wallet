//! The wallet's view of the Zcash chain.
//!
//! [`Chain`] and [`ChainView`] are the backend-neutral interface the rest of the wallet
//! uses to read chain data. The Zaino-backed implementation lives in [`zaino`].

use std::ops::Range;

use futures::stream::BoxStream;
use zcash_client_backend::data_api::{
    TransactionStatus,
    chain::{ChainState, CommitmentTreeRoot},
};
use zcash_primitives::{
    block::{Block, BlockHash, BlockHeader},
    transaction::Transaction,
};
use zcash_protocol::{TxId, consensus::BlockHeight};

mod error;
pub(crate) use error::ChainError;

mod zaino;
pub(crate) use zaino::{ZainoChain, ZainoChainView};

/// A handle to a source of Zcash chain data.
///
/// Cheap to clone; clones share the underlying source.
pub(crate) trait Chain: Clone + Send + Sync + 'static {
    /// A consistent, reorg-immune view of the chain captured by [`Chain::snapshot`].
    type View: ChainView;

    /// Broadcasts a transaction to the network's mempool.
    async fn broadcast_transaction(&self, tx: &Transaction) -> Result<(), ChainError>;

    /// Returns the Sapling note commitment subtree roots, in index order.
    async fn get_sapling_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<sapling::Node>>, ChainError>;

    /// Returns the Orchard note commitment subtree roots, in index order.
    async fn get_orchard_subtree_roots(
        &self,
    ) -> Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, ChainError>;

    /// Captures a consistent view of the chain as of the current tip.
    ///
    /// Every read through the returned [`ChainView`] reflects one fixed chain history for
    /// the lifetime of the view, regardless of reorgs or new blocks observed afterward.
    async fn snapshot(&self) -> Result<Self::View, ChainError>;
}

/// A consistent, reorg-immune view of the chain as of a fixed tip.
///
/// A sequence of reads through one `ChainView` is mutually consistent.
pub(crate) trait ChainView: Clone + Send + Sync + 'static {
    /// Returns this view's chain tip.
    async fn tip(&self) -> Result<ChainBlock, ChainError>;

    /// Returns the most recent ancestor of the caller's chain (identified by `known_tip`)
    /// that lies on this view's best chain, or `None` if it cannot be located.
    async fn find_fork_point(
        &self,
        known_tip: &BlockHash,
    ) -> Result<Option<ChainBlock>, ChainError>;

    /// Returns the final note commitment tree state for each shielded pool as of `height`,
    /// or `None` if `height` is above this view's tip.
    async fn tree_state_as_of(&self, height: BlockHeight)
    -> Result<Option<ChainState>, ChainError>;

    /// Returns the block header at `height`, or `None` if above this view's tip.
    async fn get_block_header(
        &self,
        height: BlockHeight,
    ) -> Result<Option<BlockHeader>, ChainError>;

    /// Returns the block at `height`, or `None` if above this view's tip.
    async fn get_block(&self, height: BlockHeight) -> Result<Option<Block>, ChainError>;

    /// Streams blocks from `start` to this view's tip, inclusive.
    fn stream_blocks_to_tip(&self, start: BlockHeight) -> BoxStream<'_, Result<Block, ChainError>>;

    /// Streams blocks over `range`.
    fn stream_blocks(&self, range: &Range<BlockHeight>)
    -> BoxStream<'_, Result<Block, ChainError>>;

    /// Streams the current mempool. The stream ends when this view's tip changes.
    ///
    /// Returns `None` if the tip has already changed since the view was captured.
    async fn get_mempool_stream(&self) -> Result<Option<BoxStream<'_, Transaction>>, ChainError>;

    /// Returns the transaction with the given txid, if known.
    async fn get_transaction(&self, txid: TxId) -> Result<Option<ChainTx>, ChainError>;

    /// Returns the current status of the given transaction.
    async fn get_transaction_status(&self, txid: TxId) -> Result<TransactionStatus, ChainError>;

    /// Returns the height of the given block if it is on this view's main chain.
    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    async fn block_height(&self, hash: &BlockHash) -> Result<Option<BlockHeight>, ChainError>;
}

/// A block's height and hash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChainBlock {
    pub(crate) height: BlockHeight,
    pub(crate) hash: BlockHash,
}

/// A transaction together with the chain metadata the wallet needs to ingest it.
pub(crate) struct ChainTx {
    pub(crate) inner: Transaction,
    pub(crate) raw: Vec<u8>,
    pub(crate) block_hash: Option<BlockHash>,
    pub(crate) mined_height: Option<BlockHeight>,
    pub(crate) block_time: Option<u32>,
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use futures::{
        StreamExt as _,
        stream::{self, BoxStream},
    };
    use zcash_client_backend::data_api::{TransactionStatus, chain::ChainState};
    use zcash_primitives::{
        block::{Block, BlockHash, BlockHeader},
        transaction::Transaction,
    };
    use zcash_protocol::{TxId, consensus::BlockHeight};

    use super::{ChainBlock, ChainError, ChainTx, ChainView};

    /// A trivial in-memory [`ChainView`], proving the trait is implementable by a non-Zaino
    /// backend and locking the contract.
    #[derive(Clone)]
    struct MockChainView {
        tip: ChainBlock,
    }

    impl ChainView for MockChainView {
        async fn tip(&self) -> Result<ChainBlock, ChainError> {
            Ok(self.tip)
        }

        async fn find_fork_point(
            &self,
            _known_tip: &BlockHash,
        ) -> Result<Option<ChainBlock>, ChainError> {
            Ok(Some(self.tip))
        }

        async fn tree_state_as_of(
            &self,
            _height: BlockHeight,
        ) -> Result<Option<ChainState>, ChainError> {
            Ok(None)
        }

        async fn get_block_header(
            &self,
            _height: BlockHeight,
        ) -> Result<Option<BlockHeader>, ChainError> {
            Ok(None)
        }

        async fn get_block(&self, _height: BlockHeight) -> Result<Option<Block>, ChainError> {
            Ok(None)
        }

        fn stream_blocks_to_tip(
            &self,
            _start: BlockHeight,
        ) -> BoxStream<'_, Result<Block, ChainError>> {
            stream::empty().boxed()
        }

        fn stream_blocks(
            &self,
            _range: &Range<BlockHeight>,
        ) -> BoxStream<'_, Result<Block, ChainError>> {
            stream::empty().boxed()
        }

        async fn get_mempool_stream(
            &self,
        ) -> Result<Option<BoxStream<'_, Transaction>>, ChainError> {
            Ok(None)
        }

        async fn get_transaction(&self, _txid: TxId) -> Result<Option<ChainTx>, ChainError> {
            Ok(None)
        }

        async fn get_transaction_status(
            &self,
            _txid: TxId,
        ) -> Result<TransactionStatus, ChainError> {
            Ok(TransactionStatus::TxidNotRecognized)
        }

        #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
        async fn block_height(&self, _hash: &BlockHash) -> Result<Option<BlockHeight>, ChainError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn mock_view_reports_its_tip() {
        let tip = ChainBlock {
            height: BlockHeight::from_u32(42),
            hash: BlockHash([7u8; 32]),
        };
        let view = MockChainView { tip };
        assert_eq!(view.tip().await.unwrap(), tip);
        assert_eq!(view.find_fork_point(&tip.hash).await.unwrap(), Some(tip));
    }
}
