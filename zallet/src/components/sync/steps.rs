use jsonrpsee::tracing::info;
use orchard::tree::MerkleHashOrchard;
use zaino_fetch::jsonrpc::{error::JsonRpcConnectorError, response::GetBlockResponse};
use zaino_state::fetch::FetchServiceSubscriber;
use zcash_client_backend::{
    data_api::{
        chain::{ChainState, CommitmentTreeRoot},
        scanning::ScanRange,
        WalletCommitmentTrees,
    },
    proto::compact_formats::{
        ChainMetadata, CompactBlock, CompactOrchardAction, CompactSaplingOutput,
        CompactSaplingSpend, CompactTx,
    },
};
use zcash_primitives::{block::BlockHash, merkle_tree::read_frontier_v0};
use zcash_protocol::consensus::BlockHeight;

use crate::components::database::DbConnection;

use super::SyncError;

// TODO: This type, or something similar, should be part of Zaino or Zebra.
#[derive(Clone, Copy, Debug)]
pub(super) struct ChainBlock {
    pub(super) height: BlockHeight,
    pub(super) hash: BlockHash,
    /// The hash of this block's parent.
    ///
    /// Invariant: this is `None` if and only if `height` is [`consensus::H0`].
    ///
    /// [`consensus::H0`]: zcash_protocol::consensus::H0
    pub(super) prev_hash: Option<BlockHash>,
}

impl PartialEq for ChainBlock {
    fn eq(&self, other: &Self) -> bool {
        self.height == other.height && self.hash == other.hash
    }
}

impl ChainBlock {
    pub(super) async fn resolve(
        chain: &FetchServiceSubscriber,
        hash: BlockHash,
    ) -> Result<Self, SyncError> {
        Self::resolve_inner(chain, hash.to_string()).await
    }

    pub(super) async fn tip(chain: &FetchServiceSubscriber) -> Result<Self, SyncError> {
        Self::resolve_inner(chain, "-1".into()).await
    }

    async fn resolve_inner(
        chain: &FetchServiceSubscriber,
        hash_or_height: String,
    ) -> Result<Self, SyncError> {
        // TODO: https://github.com/zingolabs/zaino/issues/249
        match chain.fetcher.get_block(hash_or_height, None).await? {
            GetBlockResponse::Raw(_) => unreachable!("We requested verbosity 1"),
            GetBlockResponse::Object {
                hash,
                height,
                previous_block_hash,
                ..
            } => Ok(Self {
                height: height
                    .map(|h| BlockHeight::from_u32(h.0))
                    .unwrap_or_else(|| todo!()),
                hash: BlockHash(hash.0 .0),
                prev_hash: previous_block_hash.map(|h| BlockHash(h.0 .0)),
            }),
        }
    }
}

pub(super) async fn update_subtree_roots(
    chain: &FetchServiceSubscriber,
    db_data: &mut DbConnection,
) -> Result<(), SyncError> {
    let sapling_roots = chain
        .fetcher
        .get_subtrees_by_index("sapling".into(), 0, None)
        .await?
        .subtrees
        .into_iter()
        .map(|subtree| {
            let mut root_hash = [0; 32];
            hex::decode_to_slice(&subtree.root, &mut root_hash).map_err(|e| {
                JsonRpcConnectorError::JsonRpcClientError(format!("Invalid subtree root: {}", e))
            })?;
            Ok(CommitmentTreeRoot::from_parts(
                BlockHeight::from_u32(subtree.end_height.0),
                sapling::Node::from_bytes(root_hash).unwrap(),
            ))
        })
        .collect::<Result<Vec<_>, SyncError>>()?;

    info!("Sapling tree has {} subtrees", sapling_roots.len());
    db_data.put_sapling_subtree_roots(0, &sapling_roots)?;

    let orchard_roots = chain
        .fetcher
        .get_subtrees_by_index("orchard".into(), 0, None)
        .await?
        .subtrees
        .into_iter()
        .map(|subtree| {
            let mut root_hash = [0; 32];
            hex::decode_to_slice(&subtree.root, &mut root_hash).map_err(|e| {
                JsonRpcConnectorError::JsonRpcClientError(format!("Invalid subtree root: {}", e))
            })?;
            Ok(CommitmentTreeRoot::from_parts(
                BlockHeight::from_u32(subtree.end_height.0),
                MerkleHashOrchard::from_bytes(&root_hash).unwrap(),
            ))
        })
        .collect::<Result<Vec<_>, SyncError>>()?;

    info!("Orchard tree has {} subtrees", orchard_roots.len());
    db_data.put_orchard_subtree_roots(0, &orchard_roots)?;

    Ok(())
}

pub(super) async fn get_chain_tip(chain: &FetchServiceSubscriber) -> Result<ChainBlock, SyncError> {
    ChainBlock::tip(chain).await
}

pub(super) async fn find_fork(
    chain: &FetchServiceSubscriber,
    mut prev_tip: ChainBlock,
    mut current_tip: ChainBlock,
) -> Result<ChainBlock, SyncError> {
    // Roll tips backwards until they have the same height.
    while prev_tip.height > current_tip.height {
        prev_tip =
            ChainBlock::resolve(chain, prev_tip.prev_hash.expect("present by invariant")).await?;
    }
    while current_tip.height > prev_tip.height {
        current_tip =
            ChainBlock::resolve(chain, current_tip.prev_hash.expect("present by invariant"))
                .await?;
    }

    // Roll tips backwards until they are the same block.
    while prev_tip != current_tip {
        // We are fetching blocks from the same data source, and we assume here that the
        // data source has one single block with no parent (the genesis block). Therefore
        // if the blocks are not currently equal, they cannot be the genesis block, and so
        // their parent hashes exist (per the `ChainBlock` invariant).
        prev_tip =
            ChainBlock::resolve(chain, prev_tip.prev_hash.expect("present by invariant")).await?;
        current_tip =
            ChainBlock::resolve(chain, current_tip.prev_hash.expect("present by invariant"))
                .await?;
    }

    // We've found the common ancestor.
    Ok(current_tip)
}

/// Fetches the given block range.
///
/// This function only fetches blocks within the main chain, and should only be given a
/// range within the finalized chain state (where heights map 1:1 with blocks).
pub(super) async fn fetch_blocks(
    chain: &FetchServiceSubscriber,
    scan_range: &ScanRange,
) -> Result<Vec<CompactBlock>, SyncError> {
    info!("Fetching blocks in range {}", scan_range);

    let mut blocks = Vec::with_capacity(scan_range.len());
    for height in u32::from(scan_range.block_range().start)..u32::from(scan_range.block_range().end)
    {
        blocks.push(fetch_block_inner(chain, height.to_string()).await?);
    }

    Ok(blocks)
}

pub(super) async fn fetch_block(
    chain: &FetchServiceSubscriber,
    hash: BlockHash,
) -> Result<CompactBlock, SyncError> {
    info!("Fetching block {}", hash);
    fetch_block_inner(chain, hash.to_string()).await
}

// TODO: Switch to fetching full blocks.
async fn fetch_block_inner(
    chain: &FetchServiceSubscriber,
    hash_or_height: String,
) -> Result<CompactBlock, SyncError> {
    let compact_block = chain.block_cache.get_compact_block(hash_or_height).await?;

    Ok(CompactBlock {
        proto_version: compact_block.proto_version,
        height: compact_block.height,
        hash: compact_block.hash,
        prev_hash: compact_block.prev_hash,
        time: compact_block.time,
        header: compact_block.header,
        vtx: compact_block
            .vtx
            .into_iter()
            .map(|ctx| CompactTx {
                index: ctx.index,
                hash: ctx.hash,
                fee: ctx.fee,
                spends: ctx
                    .spends
                    .into_iter()
                    .map(|s| CompactSaplingSpend { nf: s.nf })
                    .collect(),
                outputs: ctx
                    .outputs
                    .into_iter()
                    .map(|o| CompactSaplingOutput {
                        cmu: o.cmu,
                        ephemeral_key: o.ephemeral_key,
                        ciphertext: o.ciphertext,
                    })
                    .collect(),
                actions: ctx
                    .actions
                    .into_iter()
                    .map(|a| CompactOrchardAction {
                        nullifier: a.nullifier,
                        cmx: a.cmx,
                        ephemeral_key: a.ephemeral_key,
                        ciphertext: a.ciphertext,
                    })
                    .collect(),
            })
            .collect(),
        chain_metadata: compact_block.chain_metadata.map(|m| ChainMetadata {
            sapling_commitment_tree_size: m.sapling_commitment_tree_size,
            orchard_commitment_tree_size: m.orchard_commitment_tree_size,
        }),
    })
}

pub(super) async fn fetch_chain_state(
    chain: &FetchServiceSubscriber,
    height: BlockHeight,
) -> Result<ChainState, SyncError> {
    let tree_state = chain.fetcher.get_treestate(height.to_string()).await?;

    Ok(ChainState::new(
        BlockHeight::from_u32(
            tree_state
                .height
                .try_into()
                .expect("blocks in main chain never have height -1"),
        ),
        {
            let mut block_hash = [0; 32];
            hex::decode_to_slice(&tree_state.hash, &mut block_hash).map_err(|e| {
                JsonRpcConnectorError::JsonRpcClientError(format!("Invalid block hash: {}", e))
            })?;
            BlockHash(block_hash)
        },
        read_frontier_v0(
            hex::decode(tree_state.sapling.inner().inner())
                .map_err(|e| {
                    JsonRpcConnectorError::JsonRpcClientError(format!(
                        "Invalid Sapling tree state: {}",
                        e
                    ))
                })?
                .as_slice(),
        )
        .map_err(JsonRpcConnectorError::IoError)?,
        read_frontier_v0(
            hex::decode(tree_state.orchard.inner().inner())
                .map_err(|e| {
                    JsonRpcConnectorError::JsonRpcClientError(format!(
                        "Invalid Orchard tree state: {}",
                        e
                    ))
                })?
                .as_slice(),
        )
        .map_err(JsonRpcConnectorError::IoError)?,
    ))
}
