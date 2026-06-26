//! The wallet's view of the Zcash chain.
//!
//! [`Chain`] and [`ChainView`] are the backend-neutral interface the rest of the wallet
//! uses to read chain data. The backend implementations live in the `zaino` and `zebra`
//! modules, selected by cargo feature.

use std::future::Future;
use std::ops::Range;

use futures::stream::BoxStream;
use tracing::{error, info};
#[cfg(not(feature = "spend-index"))]
use transparent::address::TransparentAddress;
#[cfg(feature = "spend-index")]
use transparent::bundle::OutPoint;
use zcash_client_backend::data_api::{
    TransactionStatus,
    chain::{ChainState, CommitmentTreeRoot},
};
use zcash_primitives::{
    block::{Block, BlockHash, BlockHeader},
    transaction::Transaction,
};
use zcash_protocol::{
    TxId,
    consensus::{self, BlockHeight},
};

use crate::error::{Error, ErrorKind};
use crate::fl;

mod error;
pub(crate) use error::ChainError;

// Shared read-only `ReadStateService` construction, used by the `zebra-state` backend and
// by the optional read-state-service variant of the `zaino` backend.
#[cfg(any(feature = "zaino", feature = "zebra-state"))]
mod read_state;

#[cfg(feature = "zaino")]
mod zaino;
#[cfg(feature = "zaino")]
pub(crate) use zaino::ZainoChain;

#[cfg(feature = "zebra-state")]
mod zebra;
#[cfg(feature = "zebra-state")]
pub(crate) use zebra::ZebraChain;

/// The concrete chain backend selected at compile time by the `zaino` / `zebra-state`
/// feature. Construction sites name this; everything else is generic over [`Chain`].
#[cfg(feature = "zaino")]
pub(crate) type ChainBackend = ZainoChain;
#[cfg(feature = "zebra-state")]
pub(crate) type ChainBackend = ZebraChain;

/// A handle to a source of Zcash chain data.
///
/// Cheap to clone; clones share the underlying source.
pub(crate) trait Chain: Clone + Send + Sync + 'static {
    /// A consistent, reorg-immune view of the chain captured by [`Chain::snapshot`].
    type View: ChainView;

    /// The network upgrades the backing full node reports, in backend-neutral form.
    ///
    /// Consumed by [`check_consensus_compatibility`] to confirm the node’s consensus
    /// rules are compatible with this build of Zallet.
    fn reported_upgrades(&self)
    -> impl Future<Output = Result<Vec<ReportedUpgrade>, Error>> + Send;

    /// Broadcasts a transaction to the network's mempool.
    fn broadcast_transaction(
        &self,
        tx: &Transaction,
    ) -> impl Future<Output = Result<(), ChainError>> + Send;

    /// Returns the Sapling note commitment subtree roots, in index order.
    fn get_sapling_subtree_roots(
        &self,
    ) -> impl Future<Output = Result<Vec<CommitmentTreeRoot<sapling::Node>>, ChainError>> + Send;

    /// Returns the Orchard note commitment subtree roots, in index order.
    fn get_orchard_subtree_roots(
        &self,
    ) -> impl Future<
        Output = Result<Vec<CommitmentTreeRoot<orchard::tree::MerkleHashOrchard>>, ChainError>,
    > + Send;

    /// Captures a consistent view of the chain as of the current tip.
    ///
    /// Every read through the returned [`ChainView`] reflects one fixed chain history for
    /// the lifetime of the view, regardless of reorgs or new blocks observed afterward.
    fn snapshot(&self) -> impl Future<Output = Result<Self::View, ChainError>> + Send;
}

/// The status of a network upgrade as reported by a backing full node.
enum UpgradeStatus {
    /// The upgrade has activated on the node’s chain.
    Active,
    /// The upgrade is scheduled but has not yet activated.
    Pending,
    /// The upgrade has no activation height on the node’s network.
    Disabled,
}

/// A network upgrade reported by a backing full node, in a backend-neutral form.
///
/// Each [`Chain`] backend converts its own representation into this so that the
/// consensus-compatibility check ([`check_consensus_compatibility`]) is backend-neutral.
pub(crate) struct ReportedUpgrade {
    /// The consensus branch ID the node reports for this upgrade.
    branch_id: u32,
    /// The node’s name for the upgrade, used for diagnostics only.
    name: String,
    /// The activation height the node reports. Ignored when the status is
    /// [`UpgradeStatus::Disabled`], since a disabled upgrade never activates.
    activation_height: u32,
    /// Whether the node treats the upgrade as active, pending, or disabled.
    status: UpgradeStatus,
}

/// A network upgrade that the backing full node follows but that this build of
/// Zallet does not recognize.
struct UnknownUpgrade {
    /// The consensus branch ID reported by the full node.
    branch_id: u32,
    /// The full node’s name for the upgrade, used for diagnostics only.
    name: String,
    /// The activation height, if the upgrade is pending rather than already active.
    pending_at: Option<u32>,
}

impl UnknownUpgrade {
    fn describe(&self) -> String {
        match self.pending_at {
            Some(height) => format!(
                "{} (branch ID {:08x}, activates at height {height})",
                self.name, self.branch_id,
            ),
            None => format!(
                "{} (branch ID {:08x}, already active)",
                self.name, self.branch_id
            ),
        }
    }
}

/// Identifies the network upgrades reported by the backing full node whose
/// consensus branch IDs this build of Zallet cannot interpret.
///
/// Upgrades with [`UpgradeStatus::Disabled`] are ignored: they have no activation
/// height on the current network, and so will never take effect.
fn detect_unknown_upgrades(upgrades: &[ReportedUpgrade]) -> Vec<UnknownUpgrade> {
    upgrades
        .iter()
        .filter_map(|upgrade| {
            let ReportedUpgrade {
                branch_id,
                name,
                activation_height,
                status,
            } = upgrade;

            // If we can map the branch ID onto a set of consensus rules we were
            // compiled with, then we understand this upgrade.
            if consensus::BranchId::try_from(*branch_id).is_ok() {
                None
            } else {
                let unknown = |pending_at| {
                    Some(UnknownUpgrade {
                        branch_id: *branch_id,
                        name: name.clone(),
                        pending_at,
                    })
                };

                match status {
                    // Never activates on this network, so we ignore it.
                    UpgradeStatus::Disabled => None,
                    UpgradeStatus::Active => unknown(None),
                    UpgradeStatus::Pending => unknown(Some(*activation_height)),
                }
            }
        })
        .collect()
}

/// Refuses to continue if `chain`’s backing full node follows consensus rules that this
/// build of Zallet does not recognize, and so cannot interpret correctly.
pub(crate) async fn check_consensus_compatibility(chain: &impl Chain) -> Result<(), Error> {
    let upgrades = chain.reported_upgrades().await?;

    let unknown = detect_unknown_upgrades(&upgrades);
    if unknown.is_empty() {
        info!("Backing full node consensus rules are compatible with this Zallet build");
        return Ok(());
    }

    let upgrades = unknown
        .iter()
        .map(UnknownUpgrade::describe)
        .collect::<Vec<_>>()
        .join(", ");
    error!("Backing full node follows unrecognized consensus rules: {upgrades}");
    Err(ErrorKind::Init
        .context(fl!("err-init-incompatible-consensus", upgrades = upgrades))
        .into())
}

/// A consistent, reorg-immune view of the chain as of a fixed tip.
///
/// A sequence of reads through one `ChainView` is mutually consistent.
pub(crate) trait ChainView: Clone + Send + Sync + 'static {
    /// Returns this view's chain tip.
    fn tip(&self) -> impl Future<Output = Result<ChainBlock, ChainError>> + Send;

    /// Returns the most recent entry of the caller-supplied block [`BlockLocator`] that
    /// lies on this view's best chain — the fork point — or `None` if no locator entry is
    /// on the best chain.
    fn find_fork_point(
        &self,
        locator: &BlockLocator,
    ) -> impl Future<Output = Result<Option<ChainBlock>, ChainError>> + Send;

    /// Returns the final note commitment tree state for each shielded pool as of `height`,
    /// or `None` if `height` is above this view's tip.
    fn tree_state_as_of(
        &self,
        height: BlockHeight,
    ) -> impl Future<Output = Result<Option<ChainState>, ChainError>> + Send;

    /// Returns the block header at `height`, or `None` if above this view's tip.
    fn get_block_header(
        &self,
        height: BlockHeight,
    ) -> impl Future<Output = Result<Option<BlockHeader>, ChainError>> + Send;

    /// Returns the block at `height`, or `None` if above this view's tip.
    fn get_block(
        &self,
        height: BlockHeight,
    ) -> impl Future<Output = Result<Option<Block>, ChainError>> + Send;

    /// Streams blocks from `start` to this view's tip, inclusive.
    fn stream_blocks_to_tip(&self, start: BlockHeight) -> BoxStream<'_, Result<Block, ChainError>>;

    /// Streams blocks over `range`.
    fn stream_blocks(&self, range: &Range<BlockHeight>)
    -> BoxStream<'_, Result<Block, ChainError>>;

    /// Streams the current mempool. The stream ends when this view's tip changes.
    ///
    /// Returns `None` if the tip has already changed since the view was captured.
    fn get_mempool_stream(
        &self,
    ) -> impl Future<Output = Result<Option<BoxStream<'_, Transaction>>, ChainError>> + Send;

    /// Returns the transaction with the given txid, if known.
    fn get_transaction(
        &self,
        txid: TxId,
    ) -> impl Future<Output = Result<Option<ChainTx>, ChainError>> + Send;

    /// Returns the current status of the given transaction.
    fn get_transaction_status(
        &self,
        txid: TxId,
    ) -> impl Future<Output = Result<TransactionStatus, ChainError>> + Send;

    /// Returns the spend status of the transparent output `outpoint` on this view's chain.
    ///
    /// Spentness is authoritative (taken from the node's UTXO set); a per-outpoint spend index
    /// is used only to resolve the spending transaction. A spent output whose spender cannot yet
    /// be resolved is reported as [`SpendStatus::SpentSpenderUnknown`] so the caller retries
    /// rather than concluding the output is unspent (see ZcashFoundation/zebra#10806).
    #[cfg(feature = "spend-index")]
    fn outpoint_spend_status(
        &self,
        outpoint: &OutPoint,
    ) -> impl Future<Output = Result<SpendStatus, ChainError>> + Send;

    /// Returns the outpoints `(txid, output_index)` currently unspent at `address` on this
    /// view's chain, used (without a per-outpoint spend index) to cheaply decide whether any of
    /// the wallet's tracked outputs at the address have been spent.
    #[cfg(not(feature = "spend-index"))]
    fn get_address_unspent_outpoints(
        &self,
        address: &TransparentAddress,
    ) -> impl Future<Output = Result<Vec<(TxId, u32)>, ChainError>> + Send;

    /// Returns the txids of transactions involving `address` mined within `range`
    /// (start inclusive, end exclusive), used to recover the spending transaction once a missed
    /// spend has been detected on a backend without a per-outpoint spend index.
    #[cfg(not(feature = "spend-index"))]
    fn get_address_tx_ids(
        &self,
        address: &TransparentAddress,
        range: Range<BlockHeight>,
    ) -> impl Future<Output = Result<Vec<TxId>, ChainError>> + Send;

    /// Returns the height of the given block if it is on this view's main chain.
    ///
    /// Gated to the `zcashd-import` migration: its only caller resolves block hashes to
    /// heights for transactions imported from a `zcashd` wallet, so backends need not
    /// implement it in builds that cannot perform that import.
    #[cfg(all(zallet_build = "wallet", feature = "zcashd-import"))]
    fn block_height(
        &self,
        hash: &BlockHash,
    ) -> impl Future<Output = Result<Option<BlockHeight>, ChainError>> + Send;
}

/// A block's height and hash.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ChainBlock {
    pub(crate) height: BlockHeight,
    pub(crate) hash: BlockHash,
}

/// An ordered list of a caller's own block hashes, highest chain height first, used to
/// locate where the caller's chain diverges from a backend's best chain (see
/// [`ChainView::find_fork_point`]).
pub(crate) struct BlockLocator(Vec<BlockHash>);

impl BlockLocator {
    /// Builds a locator from the caller's known blocks, highest height first.
    ///
    /// # Panics
    ///
    /// Panics unless `blocks` are in strictly-decreasing height order. This is a
    /// construction invariant, not input validation: a locator must list blocks from the
    /// chain tip downward so that fork-point detection returns the *highest* shared block,
    /// and the only producer builds it from its own contiguous history — so a violation is
    /// always a programming error, caught here rather than surfacing as a silently wrong
    /// fork point.
    pub(crate) fn from_blocks(blocks: impl IntoIterator<Item = ChainBlock>) -> Self {
        let mut hashes = Vec::new();
        let mut prev_height: Option<BlockHeight> = None;
        for block in blocks {
            if let Some(prev) = prev_height {
                assert!(
                    block.height < prev,
                    "block locator heights must strictly decrease, but {} follows {}",
                    block.height,
                    prev,
                );
            }
            prev_height = Some(block.height);
            hashes.push(block.hash);
        }
        Self(hashes)
    }

    /// The locator's block hashes, highest chain height first.
    pub(crate) fn hashes(&self) -> &[BlockHash] {
        &self.0
    }
}

/// A transaction together with the chain metadata the wallet needs to ingest it.
pub(crate) struct ChainTx {
    pub(crate) inner: Transaction,
    pub(crate) raw: Vec<u8>,
    pub(crate) block_hash: Option<BlockHash>,
    pub(crate) mined_height: Option<BlockHeight>,
    pub(crate) block_time: Option<u32>,
}

/// The spend status of a transparent output, as reported by [`ChainView::outpoint_spend_status`].
#[cfg(feature = "spend-index")]
pub(crate) enum SpendStatus {
    /// The output is unspent on this view's chain.
    Unspent,
    /// The output was spent by the transaction with this txid.
    SpentBy(TxId),
    /// The output is spent, but the spending transaction cannot yet be resolved (e.g. the
    /// backend's spend index has not finished building); the caller should retry later.
    SpentSpenderUnknown,
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
    use zcash_protocol::{
        TxId,
        consensus::{BlockHeight, BranchId},
    };

    #[cfg(feature = "spend-index")]
    use super::SpendStatus;
    use super::{
        BlockLocator, ChainBlock, ChainError, ChainTx, ChainView, ReportedUpgrade, UpgradeStatus,
        detect_unknown_upgrades,
    };
    #[cfg(not(feature = "spend-index"))]
    use transparent::address::TransparentAddress;
    #[cfg(feature = "spend-index")]
    use transparent::bundle::OutPoint;

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
            locator: &BlockLocator,
        ) -> Result<Option<ChainBlock>, ChainError> {
            // The mock knows only its own tip, so the fork point is locatable only when
            // the caller's locator includes that block; otherwise it cannot be located.
            Ok(locator
                .hashes()
                .contains(&self.tip.hash)
                .then_some(self.tip))
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

        #[cfg(feature = "spend-index")]
        async fn outpoint_spend_status(
            &self,
            _outpoint: &OutPoint,
        ) -> Result<SpendStatus, ChainError> {
            Ok(SpendStatus::Unspent)
        }

        #[cfg(not(feature = "spend-index"))]
        async fn get_address_unspent_outpoints(
            &self,
            _address: &TransparentAddress,
        ) -> Result<Vec<(TxId, u32)>, ChainError> {
            Ok(Vec::new())
        }

        #[cfg(not(feature = "spend-index"))]
        async fn get_address_tx_ids(
            &self,
            _address: &TransparentAddress,
            _range: Range<BlockHeight>,
        ) -> Result<Vec<TxId>, ChainError> {
            Ok(Vec::new())
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
        // The fork point resolves when the locator includes the view's own tip, and
        // not for a locator that excludes it.
        let on_chain = BlockLocator::from_blocks([tip]);
        assert_eq!(view.find_fork_point(&on_chain).await.unwrap(), Some(tip));
        let off_chain = BlockLocator::from_blocks([ChainBlock {
            height: BlockHeight::from_u32(41),
            hash: BlockHash([0u8; 32]),
        }]);
        assert_eq!(view.find_fork_point(&off_chain).await.unwrap(), None);
    }

    fn block(height: u32, hash: u8) -> ChainBlock {
        ChainBlock {
            height: BlockHeight::from_u32(height),
            hash: BlockHash([hash; 32]),
        }
    }

    #[test]
    fn block_locator_keeps_hashes_in_descending_order() {
        let locator = BlockLocator::from_blocks([block(10, 10), block(9, 9), block(5, 5)]);
        assert_eq!(
            locator.hashes(),
            &[BlockHash([10; 32]), BlockHash([9; 32]), BlockHash([5; 32])],
        );
    }

    #[test]
    #[should_panic(expected = "strictly decrease")]
    fn block_locator_rejects_non_descending_heights() {
        // Equal heights violate the strictly-decreasing construction invariant.
        let _ = BlockLocator::from_blocks([block(10, 10), block(10, 9)]);
    }

    /// An invalid consensus branch ID, standing in for a network upgrade
    /// from the future that this build of Zallet has never heard of.
    const UNKNOWN_BRANCH_ID: u32 = 0xdead_beef;

    fn upgrade(branch_id: u32, status: UpgradeStatus) -> ReportedUpgrade {
        ReportedUpgrade {
            branch_id,
            // The upgrade name is diagnostic only; the branch ID and status drive the
            // check, so the name is fixed here.
            name: "test".into(),
            activation_height: 1,
            status,
        }
    }

    #[test]
    fn recognized_upgrade_is_compatible() {
        let known = u32::from(BranchId::Nu5);
        assert!(detect_unknown_upgrades(&[upgrade(known, UpgradeStatus::Active)]).is_empty());
    }

    #[test]
    fn unknown_active_upgrade_is_flagged() {
        let unknown = detect_unknown_upgrades(&[upgrade(UNKNOWN_BRANCH_ID, UpgradeStatus::Active)]);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].branch_id, UNKNOWN_BRANCH_ID);
        assert_eq!(unknown[0].pending_at, None);
    }

    #[test]
    fn unknown_pending_upgrade_is_flagged() {
        let unknown =
            detect_unknown_upgrades(&[upgrade(UNKNOWN_BRANCH_ID, UpgradeStatus::Pending)]);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].pending_at, Some(1));
    }

    #[test]
    fn unknown_disabled_upgrade_is_ignored() {
        assert!(
            detect_unknown_upgrades(&[upgrade(UNKNOWN_BRANCH_ID, UpgradeStatus::Disabled)])
                .is_empty()
        );
    }
}
