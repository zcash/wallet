use futures::TryStreamExt as _;
use jsonrpsee::tracing::info;
use zcash_client_backend::{
    data_api::{BlockMetadata, WalletCommitmentTrees, WalletWrite, scanning::ScanRange},
    scanning::{Nullifiers, full},
};
use zcash_client_sqlite::AccountUuid;
use zcash_primitives::block::Block;
use zip32::Scope;

use crate::{
    components::{
        chain::{Chain, ChainView},
        database::DbConnection,
    },
    error::ErrorKind,
    network::Network,
};

use super::{SyncError, decryptor};

pub(super) async fn update_subtree_roots(
    chain: &Chain,
    db_data: &mut DbConnection,
) -> Result<(), SyncError> {
    let sapling_roots = chain
        .get_sapling_subtree_roots()
        .await
        .map_err(SyncError::Chain)?;

    info!("Sapling tree has {} subtrees", sapling_roots.len());
    db_data.put_sapling_subtree_roots(0, &sapling_roots)?;

    let orchard_roots = chain
        .get_orchard_subtree_roots()
        .await
        .map_err(SyncError::Chain)?;

    info!("Orchard tree has {} subtrees", orchard_roots.len());
    db_data.put_orchard_subtree_roots(0, &orchard_roots)?;

    Ok(())
}

/// Scans a contiguous sequence of blocks in the main chain.
pub(super) async fn scan_blocks(
    chain_view: ChainView,
    db_data: &mut DbConnection,
    params: &Network,
    scan_range: &ScanRange,
    decryptor: &decryptor::Handle<AccountUuid, (AccountUuid, Scope)>,
) -> Result<(), SyncError> {
    // Ignore scan ranges beyond the end of the current chain tip (which indicates a race
    // with a chain reorg).
    if let Some(from_state) = chain_view
        .tree_state_as_of(scan_range.block_range().start - 1)
        .await
        .map_err(SyncError::Chain)?
    {
        info!("Scanning blocks {}", scan_range);
        let blocks_to_apply = chain_view.stream_blocks(scan_range.block_range());
        tokio::pin!(blocks_to_apply);

        // Queue the blocks for batch decryption.
        let mut batch = Vec::with_capacity(scan_range.len());
        while let Some(block) = blocks_to_apply.try_next().await.map_err(SyncError::Chain)? {
            let height = block.claimed_height();
            let result = decryptor
                .queue_block(block)
                .await
                .ok_or(SyncError::BatchDecryptorUnavailable)?;
            batch.push((height, result));
        }

        let mut prior_block_metadata = Some(BlockMetadata::from_parts(
            from_state.block_height(),
            from_state.block_hash(),
            Some(from_state.final_sapling_tree().tree_size() as u32),
            Some(from_state.final_orchard_tree().tree_size() as u32),
        ));

        // Get the nullifiers for the unspent notes we are tracking
        let mut nullifiers = Nullifiers::unspent(db_data)?;

        // Now wait on the batch and scan each block as it becomes available.
        let mut scanned_blocks = Vec::with_capacity(scan_range.len());
        for (height, result) in batch {
            let (scanning_keys, header, vtx) = result
                .await
                .map_err(|_| SyncError::BatchDecryptorUnavailable)?;

            let scanned_block = full::scan_block(
                params,
                height,
                &header,
                &vtx,
                &scanning_keys,
                &nullifiers,
                prior_block_metadata.as_ref(),
            )
            .map_err(SyncError::Scan)?;

            nullifiers.update_with(&scanned_block);
            prior_block_metadata = Some(scanned_block.to_block_metadata());
            scanned_blocks.push(scanned_block);
        }

        tokio::task::block_in_place(|| db_data.put_blocks(&from_state, scanned_blocks))?;
    } else {
        info!(
            "{} is greater than chain view's tip ({}), skipping",
            scan_range.block_range().start - 1,
            chain_view.tip().await.map_err(SyncError::Chain)?.height,
        );
    }

    Ok(())
}

/// Scans a block in the main chain.
pub(super) async fn scan_block(
    chain_view: &ChainView,
    db_data: &mut DbConnection,
    params: &Network,
    block: Block,
    decryptor: &decryptor::Handle<AccountUuid, (AccountUuid, Scope)>,
) -> Result<(), SyncError> {
    let height = block.claimed_height();

    let from_state = chain_view
        .tree_state_as_of(height - 1)
        .await
        .map_err(SyncError::Chain)?
        .ok_or_else(|| {
            SyncError::Chain(
                ErrorKind::Sync
                    .context("Programming error: tried to scan block ahead of the chain view's tip")
                    .into(),
            )
        })?;

    info!("Scanning block {} ({})", height, block.header().hash());
    let result = decryptor
        .queue_block(block)
        .await
        .ok_or(SyncError::BatchDecryptorUnavailable)?;

    let prior_block_metadata = Some(BlockMetadata::from_parts(
        from_state.block_height(),
        from_state.block_hash(),
        Some(from_state.final_sapling_tree().tree_size() as u32),
        Some(from_state.final_orchard_tree().tree_size() as u32),
    ));

    // Get the nullifiers for the unspent notes we are tracking
    let nullifiers = Nullifiers::unspent(db_data)?;

    let (scanning_keys, header, vtx) = result
        .await
        .map_err(|_| SyncError::BatchDecryptorUnavailable)?;

    let scanned = full::scan_block(
        params,
        height,
        &header,
        &vtx,
        &scanning_keys,
        &nullifiers,
        prior_block_metadata.as_ref(),
    )
    .map_err(SyncError::Scan)?;

    tokio::task::block_in_place(|| db_data.put_blocks(&from_state, vec![scanned]))?;

    Ok(())
}
