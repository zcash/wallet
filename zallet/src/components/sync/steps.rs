use incrementalmerkletree::frontier::Frontier;
use jsonrpsee::tracing::info;
use orchard::tree::MerkleHashOrchard;
use zaino_fetch::jsonrpsee::connector::RpcError;
use zaino_state::{
    FetchServiceError, FetchServiceSubscriber, LightWalletIndexer as _, ZcashIndexer as _,
};
use zcash_client_backend::{
    data_api::{
        WalletCommitmentTrees,
        chain::{ChainState, CommitmentTreeRoot},
        scanning::ScanRange,
    },
    proto::compact_formats::{
        ChainMetadata, CompactBlock, CompactOrchardAction, CompactSaplingOutput,
        CompactSaplingSpend, CompactTx,
    },
};
use zcash_primitives::{block::BlockHash, merkle_tree::read_frontier_v0};
use zcash_protocol::consensus::{BlockHeight, NetworkUpgrade, Parameters};
use zebra_chain::{
    block::Block, serialization::ZcashDeserialize as _, subtree::NoteCommitmentSubtreeIndex,
};
use zebra_rpc::{methods::GetBlock, server::error::LegacyCode};
use zebra_state::HashOrHeight;

use crate::{
    components::{database::DbConnection, sync::error::IndexerError},
    network::Network,
};

use super::SyncError;

// TODO: This type, or something similar, should be part of Zaino or Zebra.
// TODO: https://github.com/zingolabs/zaino/issues/249
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
        Self::resolve_inner(chain, HashOrHeight::Hash(hash.0.into())).await
    }

    pub(super) async fn tip(chain: &FetchServiceSubscriber) -> Result<Self, SyncError> {
        let block_id = chain.get_latest_block().await?;
        let compact_block = chain.get_block(block_id).await?;

        Ok(Self {
            height: BlockHeight::from_u32(compact_block.height.try_into().map_err(
                |e: std::num::TryFromIntError| FetchServiceError::SerializationError(e.into()),
            )?),
            hash: BlockHash::try_from_slice(compact_block.hash.as_slice())
                .expect("block hash missing"),
            prev_hash: BlockHash::try_from_slice(compact_block.prev_hash.as_slice()),
        })
    }

    async fn resolve_inner(
        chain: &FetchServiceSubscriber,
        hash_or_height: HashOrHeight,
    ) -> Result<Self, SyncError> {
        let mut block_id = zaino_proto::proto::service::BlockId::default();
        match hash_or_height {
            HashOrHeight::Hash(hash) => block_id.hash = hash.0.to_vec(),
            HashOrHeight::Height(height) => block_id.height = height.0 as u64,
        };

        let compact_block = chain.get_block(block_id).await?;

        Ok(Self {
            height: BlockHeight::from_u32(compact_block.height.try_into().map_err(
                |e: std::num::TryFromIntError| FetchServiceError::SerializationError(e.into()),
            )?),
            hash: BlockHash::try_from_slice(compact_block.hash.as_slice())
                .expect("block hash missing"),
            prev_hash: BlockHash::try_from_slice(compact_block.prev_hash.as_slice()),
        })
    }
}

pub(super) async fn update_subtree_roots(
    chain: &FetchServiceSubscriber,
    db_data: &mut DbConnection,
) -> Result<(), SyncError> {
    let sapling_roots = chain
        .z_get_subtrees_by_index("sapling".into(), NoteCommitmentSubtreeIndex(0), None)
        .await?
        .subtrees()
        .iter()
        .map(|subtree| {
            let mut root_hash = [0; 32];
            hex::decode_to_slice(&subtree.root, &mut root_hash).map_err(|e| {
                FetchServiceError::RpcError(RpcError::new_from_legacycode(
                    LegacyCode::Deserialization,
                    format!("Invalid subtree root: {e}"),
                ))
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
        .z_get_subtrees_by_index("orchard".into(), NoteCommitmentSubtreeIndex(0), None)
        .await?
        .subtrees()
        .iter()
        .map(|subtree| {
            let mut root_hash = [0; 32];
            hex::decode_to_slice(&subtree.root, &mut root_hash).map_err(|e| {
                FetchServiceError::RpcError(RpcError::new_from_legacycode(
                    LegacyCode::Deserialization,
                    format!("Invalid subtree root: {e}"),
                ))
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
        blocks.push(
            fetch_compact_block_inner(
                chain,
                HashOrHeight::Height(height.try_into().expect("valid")),
            )
            .await?,
        );
    }

    Ok(blocks)
}

pub(super) async fn fetch_block(
    chain: &FetchServiceSubscriber,
    hash: BlockHash,
) -> Result<CompactBlock, SyncError> {
    info!("Fetching block {}", hash);
    fetch_compact_block_inner(chain, HashOrHeight::Hash(hash.0.into())).await
}

#[allow(dead_code)]
async fn fetch_full_block_inner(
    chain: &FetchServiceSubscriber,
    hash_or_height: HashOrHeight,
) -> Result<Block, SyncError> {
    match chain
        .z_get_block(hash_or_height.to_string(), Some(0))
        .await?
    {
        GetBlock::Raw(bytes) => {
            let block = Block::zcash_deserialize(bytes.as_ref())
                .map_err(FetchServiceError::SerializationError)?;
            Ok(block)
        }
        GetBlock::Object { .. } => unreachable!("We requested verbosity 0"),
    }
}

// TODO: Switch to fetching full blocks.
async fn fetch_compact_block_inner(
    chain: &FetchServiceSubscriber,
    hash_or_height: HashOrHeight,
) -> Result<CompactBlock, SyncError> {
    let mut block_id = zaino_proto::proto::service::BlockId::default();
    match hash_or_height {
        HashOrHeight::Hash(hash) => block_id.hash = hash.0.to_vec(),
        HashOrHeight::Height(height) => block_id.height = height.0 as u64,
    };

    let compact_block = chain.get_block(block_id).await?;

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
                hash: ctx.txid,
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
    params: &Network,
    height: BlockHeight,
) -> Result<ChainState, SyncError> {
    let (hash, height, _time, sapling, orchard) = chain
        .z_get_treestate(height.to_string())
        .await?
        .into_parts();

    let final_sapling_tree = if params.is_nu_active(NetworkUpgrade::Sapling, height.0.into()) {
        read_frontier_v0(
            sapling
                .ok_or_else(|| IndexerError::InvalidData {
                    message: "Missing Sapling tree state".into(),
                })?
                .as_slice(),
        )
        .map_err(|e| IndexerError::InvalidData {
            message: format!("{e}"),
        })?
    } else {
        // Sapling is not yet active; the Sapling tree is empty.
        Frontier::empty()
    };

    let final_orchard_tree = if params.is_nu_active(NetworkUpgrade::Nu5, height.0.into()) {
        read_frontier_v0(
            orchard
                .ok_or_else(|| IndexerError::InvalidData {
                    message: "Missing Orchard tree state".into(),
                })?
                .as_slice(),
        )
        .map_err(|e| IndexerError::InvalidData {
            message: format!("{e}"),
        })?
    } else {
        // NU5 is not yet active; the Orchard tree is empty.
        Frontier::empty()
    };

    Ok(ChainState::new(
        BlockHeight::from_u32(height.0),
        BlockHash(hash.0),
        final_sapling_tree,
        final_orchard_tree,
    ))
}
