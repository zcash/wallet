use jsonrpsee::tracing::info;
use zcash_client_backend::data_api::{
    WalletCommitmentTrees, wallet::decrypt_and_store_transaction,
};
use zcash_protocol::consensus::BlockHeight;

use crate::{
    components::{
        chain::{Block, Chain},
        database::DbConnection,
    },
    network::Network,
};

use super::SyncError;

pub(super) async fn update_subtree_roots(
    chain: &Chain,
    db_data: &mut DbConnection,
) -> Result<(), SyncError> {
    let sapling_roots = chain
        .get_sapling_subtree_roots()
        .await
        .map_err(SyncError::Indexer)?;

    info!("Sapling tree has {} subtrees", sapling_roots.len());
    db_data.put_sapling_subtree_roots(0, &sapling_roots)?;

    let orchard_roots = chain
        .get_orchard_subtree_roots()
        .await
        .map_err(SyncError::Indexer)?;

    info!("Orchard tree has {} subtrees", orchard_roots.len());
    db_data.put_orchard_subtree_roots(0, &orchard_roots)?;

    Ok(())
}

/// Scans a block in the main chain.
pub(super) async fn scan_block(
    db_data: &mut DbConnection,
    params: &Network,
    height: BlockHeight,
    block: Block,
) -> Result<(), SyncError> {
    // TODO: Use batch decryption once that API is finished and exposed from zcash_client_backend.
    tokio::task::block_in_place(|| {
        info!("Scanning block {} ({})", height, block.header.hash());
        for tx in block.vtx {
            decrypt_and_store_transaction(params, db_data, &tx, Some(height))?;
        }
        // TODO: Call equivalent of `put_blocks`.
        // Err(chain::error::Error::Scan(ScanError::PrevHashMismatch { at_height })) => {
        //     db_data
        //         .truncate_to_height(at_height - 10)
        //         .map_err(chain::error::Error::Wallet)?;
        //     Ok(())
        // }
        Ok(())
    })
    .map_err(SyncError::Other)
}
