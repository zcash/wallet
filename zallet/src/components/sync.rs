use std::time::Duration;

use futures::StreamExt as _;
use jsonrpsee::tracing::{self, debug, info, warn};
use tokio::time;
use zaino_state::{fetch::FetchServiceSubscriber, indexer::LightWalletIndexer as _};
use zcash_client_backend::data_api::{
    WalletRead, WalletWrite,
    chain::{BlockCache, scan_cached_blocks},
    scanning::{ScanPriority, ScanRange},
    wallet::decrypt_and_store_transaction,
};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::{self, BlockHeight};
use zebra_chain::transaction::SerializedTransaction;

use super::{
    TaskHandle,
    chain_view::ChainView,
    database::{Database, DbConnection},
};
use crate::{config::ZalletConfig, error::Error, network::Network};

mod cache;

mod error;
pub(crate) use error::SyncError;

mod steps;
use steps::ChainBlock;

#[derive(Debug)]
pub(crate) struct WalletSync {}

impl WalletSync {
    pub(crate) async fn spawn(
        config: &ZalletConfig,
        db: Database,
        chain_view: ChainView,
    ) -> Result<(TaskHandle, TaskHandle), Error> {
        let params = config.network();

        // Ensure the wallet is in a state that the sync tasks can work with.
        let chain = chain_view.subscribe().await?.inner();
        let mut db_data = db.handle().await?;
        let starting_tip = initialize(chain, &params, db_data.as_mut()).await?;

        // Spawn the ongoing sync tasks.
        let chain = chain_view.subscribe().await?.inner();
        let steady_state_task = tokio::spawn(async move {
            steady_state(&chain, &params, db_data.as_mut(), starting_tip).await?;
            Ok(())
        });

        let chain = chain_view.subscribe().await?.inner();
        let mut db_data = db.handle().await?;
        let recover_history_task = tokio::spawn(async move {
            recover_history(chain, &params, db_data.as_mut(), 1000).await?;
            Ok(())
        });

        Ok((steady_state_task, recover_history_task))
    }
}

/// Prepares the wallet state for syncing.
///
/// Returns the boundary block between [`steady_state`] and [`recover_history`] syncing.
#[tracing::instrument(skip_all)]
async fn initialize(
    chain: FetchServiceSubscriber,
    params: &Network,
    db_data: &mut DbConnection,
) -> Result<ChainBlock, SyncError> {
    info!("Initializing wallet for syncing");

    // Notify the wallet of the current subtree roots.
    steps::update_subtree_roots(&chain, db_data).await?;

    // Notify the wallet of the current chain tip.
    let current_tip = steps::get_chain_tip(&chain).await?;
    info!("Latest block height is {}", current_tip.height);
    db_data.update_chain_tip(current_tip.height)?;

    // TODO: Remove this once we've made `zcash_client_sqlite` changes to support scanning
    // regular blocks.
    let db_cache = cache::MemoryCache::new();

    // Detect reorgs that might have occurred while the wallet was offline, by explicitly
    // syncing any `ScanPriority::Verify` ranges. This ensures that `recover_history` only
    // operates over the finalized chain state and doesn't attempt to handle reorgs (which
    // are the responsibility of `steady_state`).
    loop {
        // If there is a range of blocks that needs to be verified, it will always be
        // returned as the first element of the vector of suggested ranges.
        let scan_range = match db_data.suggest_scan_ranges()?.into_iter().next() {
            Some(r) if r.priority() == ScanPriority::Verify => r,
            _ => break,
        };

        db_cache
            .insert(steps::fetch_blocks(&chain, &scan_range).await?)
            .await?;

        let from_state =
            steps::fetch_chain_state(&chain, scan_range.block_range().start - 1).await?;

        // Scan the downloaded blocks.
        tokio::task::block_in_place(|| {
            info!("Scanning {}", scan_range);
            scan_cached_blocks(
                params,
                &db_cache,
                db_data,
                scan_range.block_range().start,
                &from_state,
                scan_range.len(),
            )
        })?;

        // Delete the now-scanned blocks.
        db_cache.delete(scan_range).await?;
    }

    info!(
        "Initial boundary between recovery and steady-state sync is {} {}",
        current_tip.height, current_tip.hash
    );
    Ok(current_tip)
}

/// Keeps the wallet state up-to-date with the chain tip, and handles the mempool.
#[tracing::instrument(skip_all)]
async fn steady_state(
    chain: &FetchServiceSubscriber,
    params: &Network,
    db_data: &mut DbConnection,
    mut prev_tip: ChainBlock,
) -> Result<(), SyncError> {
    info!("Steady-state sync task started");
    let mut current_tip = steps::get_chain_tip(chain).await?;

    // TODO: Remove this once we've made `zcash_client_sqlite` changes to support scanning
    // regular blocks.
    let db_cache = cache::MemoryCache::new();

    loop {
        info!("New chain tip: {} {}", current_tip.height, current_tip.hash);

        // Figure out the diff between the previous and current chain tips.
        let fork_point = steps::find_fork(chain, prev_tip, current_tip).await?;
        assert!(fork_point.height <= current_tip.height);

        // Fetch blocks that need to be applied to the wallet.
        let mut block_stack =
            Vec::with_capacity((current_tip.height - fork_point.height).try_into().unwrap());
        {
            let mut current_block = current_tip;
            while current_block != fork_point {
                block_stack.push(steps::fetch_block(chain, current_block.hash).await?);
                current_block = ChainBlock::resolve(
                    chain,
                    current_block.prev_hash.expect("present by invariant"),
                )
                .await?;
            }
        }

        // If the fork point is equal to `prev_tip` then no reorg has occurred.
        if fork_point != prev_tip {
            // Ensured by `find_fork`.
            assert!(fork_point.height < prev_tip.height);

            // Rewind the wallet to the fork point.
            // TODO: Is there anything else we should do with the blocks in the old fork?
            info!(
                "Chain reorg detected, rewinding to {} {}",
                fork_point.height, fork_point.hash
            );
            db_data.truncate_to_height(fork_point.height)?;
        }

        // Notify the wallet of block connections.
        db_data.update_chain_tip(current_tip.height)?;
        if !block_stack.is_empty() {
            let from_height =
                BlockHeight::from_u32(block_stack.last().expect("not empty").height as u32);
            let end_height =
                BlockHeight::from_u32(block_stack.first().expect("not empty").height as u32 + 1);
            let scan_range = ScanRange::from_parts(from_height..end_height, ScanPriority::ChainTip);
            db_cache.insert(block_stack).await?;

            let from_state = steps::fetch_chain_state(chain, from_height.saturating_sub(1)).await?;

            tokio::task::block_in_place(|| {
                info!("Scanning {}", scan_range);
                scan_cached_blocks(
                    params,
                    &db_cache,
                    db_data,
                    from_height,
                    &from_state,
                    scan_range.len(),
                )
            })?;

            db_cache.delete(scan_range).await?;
        }

        // Now that we're done applying the chain diff, update our chain pointers.
        prev_tip = current_tip;
        current_tip = steps::get_chain_tip(chain).await?;

        // If the chain tip no longer matches, we have more to do before consuming mempool
        // updates.
        if prev_tip != current_tip {
            continue;
        }

        // We have caught up to the chain tip. Stream the mempool state into the wallet.
        info!("Reached chain tip, streaming mempool");
        let mempool_height = current_tip.height + 1;
        let consensus_branch_id = consensus::BranchId::for_height(params, mempool_height);
        let mut mempool_stream = chain.get_mempool_stream().await?;
        while let Some(result) = mempool_stream.next().await {
            match result {
                Ok(raw_tx) => {
                    let tx = Transaction::read(
                        SerializedTransaction::from(raw_tx.data).as_ref(),
                        consensus_branch_id,
                    )
                    .expect("Zaino should only provide valid transactions");
                    info!("Scanning mempool tx {}", tx.txid());
                    decrypt_and_store_transaction(params, db_data, &tx, None)?;
                }
                Err(e) => {
                    warn!("Error receiving transaction: {}", e);
                    // return error here?
                }
            }
        }

        // Mempool stream ended, signalling that the chain tip has changed.
        current_tip = steps::get_chain_tip(chain).await?;
    }
}

/// Recovers historic wallet state.
///
/// This function only operates on finalized chain state, and does not handle reorgs.
#[tracing::instrument(skip_all)]
async fn recover_history(
    chain: FetchServiceSubscriber,
    params: &Network,
    db_data: &mut DbConnection,
    batch_size: u32,
) -> Result<(), SyncError> {
    info!("History recovery sync task started");
    // TODO: Remove this once we've made `zcash_client_sqlite` changes to support scanning
    // regular blocks.
    let db_cache = cache::MemoryCache::new();

    let mut interval = time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    // The first tick completes immediately. We want to use it for a conditional delay, so
    // get that out of the way.
    interval.tick().await;

    loop {
        // Get the next suggested scan range. We drop the rest because we re-fetch the
        // entire list regularly.
        let scan_range = match db_data.suggest_scan_ranges()?.into_iter().next() {
            Some(r) => r,
            None => {
                // Wait for scan ranges to become available.
                debug!("No scan ranges, sleeping");
                interval.tick().await;
                continue;
            }
        };

        // Limit the number of blocks we download and scan at any one time.
        for scan_range in (0..).scan(Some(scan_range), |acc, _| {
            acc.clone().map(|remaining| {
                if let Some((cur, next)) =
                    remaining.split_at(remaining.block_range().start + batch_size)
                {
                    *acc = Some(next);
                    cur
                } else {
                    *acc = None;
                    remaining
                }
            })
        }) {
            db_cache
                .insert(steps::fetch_blocks(&chain, &scan_range).await?)
                .await?;

            let from_state =
                steps::fetch_chain_state(&chain, scan_range.block_range().start - 1).await?;

            // Scan the downloaded blocks.
            tokio::task::block_in_place(|| {
                info!("Scanning {}", scan_range);
                scan_cached_blocks(
                    params,
                    &db_cache,
                    db_data,
                    scan_range.block_range().start,
                    &from_state,
                    scan_range.len(),
                )
            })?;

            // If scanning these blocks caused a suggested range to be added that has a
            // higher priority than the current range, invalidate the current ranges.
            let latest_ranges = db_data.suggest_scan_ranges()?;
            let scan_ranges_updated = latest_ranges
                .first()
                .is_some_and(|range| range.priority() > scan_range.priority());

            // Delete the now-scanned blocks.
            db_cache.delete(scan_range).await?;

            if scan_ranges_updated {
                break;
            }
        }
    }
}
