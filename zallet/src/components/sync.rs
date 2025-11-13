//! The Zallet sync engine.
//!
//! # Design
//!
//! Zallet uses `zcash_client_sqlite` for its wallet, which stores its own view of the
//! chain. The goal of this engine is to keep the wallet's chain view as closely synced to
//! the network's chain as possible. This means handling environmental events such as:
//!
//! - A new block being mined.
//! - A reorg to a different chain.
//! - A transaction being added to the mempool.
//! - A new viewing capability being added to the wallet.
//! - The wallet starting up after being offline for some time.
//!
//! To handle this, we split the chain into two "regions of responsibility":
//!
//! - The [`steady_state`] task handles the region of the chain within 100 blocks of the
//!   network chain tip (corresponding to Zebra's "non-finalized state"). This task is
//!   started once when Zallet starts, and any error will cause Zallet to shut down.
//! - The [`recover_history`] task handles the region of the chain farther than 100 blocks
//!   from the network chain tip (corresponding to Zebra's "finalized state"). This task
//!   is active whenever there are unscanned blocks in this region.
//!
//! Note the boundary between these regions may be less than 100 blocks from the network
//! chain tip at times, due to how reorgs are implemented in Zebra; the boundary ratchets
//! forward as the chain tip height increases, but never backwards.
//!
//! TODO: Integrate or remove these other notes:
//!
//! - Zebra discards the non-finalized chain tip on restart, so Zallet needs to tolerate
//!   the `ChainView` being up to 100 blocks behind the wallet's view of the chain tip at
//!   process start.

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use futures::{StreamExt as _, TryStreamExt as _};
use jsonrpsee::tracing::{self, debug, info};
use tokio::{sync::Notify, time};
use zcash_client_backend::{
    data_api::{
        TransactionDataRequest, TransactionStatus, WalletRead, WalletWrite, scanning::ScanPriority,
        wallet::decrypt_and_store_transaction,
    },
    scanning::ScanningKeys,
    sync::decryptor,
};
use zcash_client_sqlite::AccountUuid;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;
use zip32::Scope;

use super::{
    TaskHandle,
    chain::{Chain, ChainBlock},
    database::{Database, DbConnection},
};
use crate::{config::ZalletConfig, error::Error, network::Network};

mod error;
pub(crate) use error::SyncError;

mod steps;

#[derive(Debug)]
pub(crate) struct WalletSync {}

impl WalletSync {
    pub(crate) async fn spawn(
        config: &ZalletConfig,
        db: Database,
        chain: Chain,
    ) -> Result<(TaskHandle, TaskHandle, TaskHandle, TaskHandle), Error> {
        let params = config.consensus.network();

        // TODO: Make configurable?
        let decryptor_queue_size = 1000;
        let decryptor_batch_size_threshold = 200;
        let decryptor_batch_start_delay = Duration::from_millis(500);
        let recover_batch_size = 1000;

        let (decryptor, decryptor_engine) = decryptor::new()
            .queue_size(decryptor_queue_size)
            .batch_size_threshold(decryptor_batch_size_threshold)
            .batch_start_delay(decryptor_batch_start_delay)
            .build();

        // Spawn the processing tasks.
        let batch_decryptor_task = {
            let mut db_data = db.handle().await?;
            crate::spawn!("Batch decryptor", async move {
                batch_decryptor(params, db_data.as_mut(), decryptor_engine).await?;
                Ok(())
            })
        };

        // Ensure the wallet is in a state that the sync tasks can work with.
        let mut db_data = db.handle().await?;
        let (starting_tip, starting_boundary) =
            initialize(&chain, &params, db_data.as_mut(), decryptor.clone()).await?;

        // Manage the boundary between the `steady_state` and `recover_history` tasks with
        // an atomic.
        let current_boundary = Arc::new(AtomicU32::new(starting_boundary.into()));

        // TODO: Zaino should provide us an API that allows us to be notified when the chain tip
        // changes; here, we produce our own signal via the "mempool stream closing" side effect
        // that occurs in the light client API when the chain tip changes.
        let tip_change_signal_source = Arc::new(Notify::new());
        let req_tip_change_signal_receiver = tip_change_signal_source.clone();

        // Spawn the ongoing sync tasks.
        let steady_state_task = {
            let chain = chain.clone();
            let lower_boundary = current_boundary.clone();
            let decryptor = decryptor.clone();
            crate::spawn!("Steady state sync", async move {
                steady_state(
                    chain,
                    &params,
                    db_data.as_mut(),
                    starting_tip,
                    lower_boundary,
                    tip_change_signal_source,
                    decryptor,
                )
                .await?;
                Ok(())
            })
        };

        let recover_history_task = {
            let chain = chain.clone();
            let mut db_data = db.handle().await?;
            let upper_boundary = current_boundary.clone();
            crate::spawn!("Recover history", async move {
                recover_history(
                    chain,
                    &params,
                    db_data.as_mut(),
                    upper_boundary,
                    decryptor,
                    recover_batch_size,
                )
                .await?;
                Ok(())
            })
        };

        let mut db_data = db.handle().await?;
        let data_requests_task = crate::spawn!("Data requests", async move {
            data_requests(
                chain,
                &params,
                db_data.as_mut(),
                req_tip_change_signal_receiver,
            )
            .await?;
            Ok(())
        });

        Ok((
            batch_decryptor_task,
            steady_state_task,
            recover_history_task,
            data_requests_task,
        ))
    }
}

fn update_boundary(current_boundary: BlockHeight, tip_height: BlockHeight) -> BlockHeight {
    current_boundary.max(tip_height - 100)
}

/// Prepares the wallet state for syncing.
///
/// Returns the boundary block between [`steady_state`] and [`recover_history`] syncing.
#[tracing::instrument(skip_all)]
async fn initialize(
    chain: &Chain,
    params: &Network,
    db_data: &mut DbConnection,
    decryptor: decryptor::Handle<AccountUuid, (AccountUuid, Scope)>,
) -> Result<(ChainBlock, BlockHeight), SyncError> {
    info!("Initializing wallet for syncing");

    // Notify the wallet of the current subtree roots.
    steps::update_subtree_roots(&chain, db_data).await?;

    // Perform initial scanning prior to firing off the main tasks:
    // - Detect reorgs that might have occurred while the wallet was offline, by
    //   explicitly syncing any `ScanPriority::Verify` ranges.
    // - Ensure that the `steady_state` task starts from the wallet's view of the chain
    //   tip, by explicitly syncing any unscanned ranges from the boundary onward.
    //
    // This ensures that the `recover_history` task only operates over the finalized chain
    // state and doesn't attempt to handle reorgs (which are the responsibility of the
    // `steady_state` task).
    let (current_tip, starting_boundary) = loop {
        // Notify the wallet of the current chain tip.
        let chain_view = chain.snapshot();
        let current_tip = chain_view.tip().await.map_err(SyncError::Chain)?;
        info!("Latest block height is {}", current_tip.height);
        db_data.update_chain_tip(current_tip.height)?;

        // Set the starting boundary between the `steady_state` and `recover_history` tasks.
        let starting_boundary = update_boundary(BlockHeight::from_u32(0), current_tip.height);

        let scan_range = match db_data
            .suggest_scan_ranges()?
            .into_iter()
            .filter_map(|r| {
                if r.priority() == ScanPriority::Verify {
                    Some(r)
                } else if r.priority() >= ScanPriority::Historic {
                    r.truncate_start(starting_boundary)
                } else {
                    None
                }
            })
            .next()
        {
            Some(r) => r,
            None => break (current_tip, starting_boundary),
        };

        steps::scan_blocks(chain_view, db_data, params, &scan_range, &decryptor).await?;
    };

    info!(
        "Initial boundary between recovery and steady-state sync is {}",
        starting_boundary,
    );
    Ok((current_tip, starting_boundary))
}

/// Keeps the wallet state up-to-date with the chain tip, and handles the mempool.
#[tracing::instrument(skip_all)]
async fn steady_state(
    chain: Chain,
    params: &Network,
    db_data: &mut DbConnection,
    mut prev_tip: ChainBlock,
    lower_boundary: Arc<AtomicU32>,
    tip_change_signal: Arc<Notify>,
    decryptor: decryptor::Handle<AccountUuid, (AccountUuid, Scope)>,
) -> Result<(), SyncError> {
    info!("Steady-state sync task started");

    loop {
        let chain_view = chain.snapshot();
        let current_tip = chain_view.tip().await.map_err(SyncError::Chain)?;

        info!("New chain tip: {} {}", current_tip.height, current_tip.hash);
        lower_boundary
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current_boundary| {
                Some(
                    update_boundary(BlockHeight::from_u32(current_boundary), current_tip.height)
                        .into(),
                )
            })
            .expect("closure always returns Some");
        tip_change_signal.notify_one();

        // Figure out the diff between the previous and current chain tips.
        let fork_point = chain_view
            .find_fork_point(&prev_tip.hash)
            .await
            .map_err(SyncError::Chain)?
            .unwrap_or_else(|| ChainBlock {
                height: BlockHeight::from_u32(0),
                // TODO: Get genesis block hash from somewhere.
                hash: BlockHash::from_slice(&[]),
            });
        assert!(fork_point.height <= current_tip.height);

        // Fetch blocks that need to be applied to the wallet.
        let blocks_to_apply = chain_view.stream_blocks_to_tip(fork_point.height + 1);
        tokio::pin!(blocks_to_apply);

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
            prev_tip = fork_point;
        };

        // Notify the wallet of block connections.
        while let Some(block) = blocks_to_apply.try_next().await.map_err(SyncError::Chain)? {
            let height = block.claimed_height();
            assert_eq!(height, prev_tip.height + 1);
            let current_tip = ChainBlock {
                height,
                hash: block.header().hash(),
            };
            db_data.update_chain_tip(height)?;

            steps::scan_block(&chain_view, db_data, params, block, &decryptor).await?;

            // Now that we're done applying the block, update our chain pointer.
            prev_tip = current_tip;
        }

        // If we have caught up to the chain tip, stream the mempool state into the wallet.
        if let Some(mempool_stream) = chain_view
            .get_mempool_stream()
            .await
            .map_err(SyncError::Chain)?
        {
            info!("Reached chain tip, streaming mempool");
            tokio::pin!(mempool_stream);
            while let Some(tx) = mempool_stream.next().await {
                info!("Scanning mempool tx {}", tx.txid());
                // TODO: Use batch decryptor for mempool scanning.
                decrypt_and_store_transaction(params, db_data, &tx, None)?;
            }

            // Mempool stream ended, signalling that the chain tip has changed.
        }
    }
}

/// Recovers historic wallet state.
///
/// This function only operates on finalized chain state, and does not handle reorgs.
#[tracing::instrument(skip_all)]
async fn recover_history(
    chain: Chain,
    params: &Network,
    db_data: &mut DbConnection,
    upper_boundary: Arc<AtomicU32>,
    decryptor: decryptor::Handle<AccountUuid, (AccountUuid, Scope)>,
    batch_size: u32,
) -> Result<(), SyncError> {
    info!("History recovery sync task started");

    let mut interval = time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    // The first tick completes immediately. We want to use it for a conditional delay, so
    // get that out of the way.
    interval.tick().await;

    loop {
        // Get the next suggested scan range. We drop the rest because we re-fetch the
        // entire list regularly.
        let upper_boundary = BlockHeight::from_u32(upper_boundary.load(Ordering::Acquire));
        let scan_range = match db_data
            .suggest_scan_ranges()?
            .into_iter()
            .filter_map(|r| r.truncate_end(upper_boundary))
            .next()
        {
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
            steps::scan_blocks(chain.snapshot(), db_data, params, &scan_range, &decryptor).await?;

            // If scanning these blocks caused a suggested range to be added that has a
            // higher priority than the current range, invalidate the current ranges.
            let latest_ranges = db_data.suggest_scan_ranges()?;
            let scan_ranges_updated = latest_ranges
                .first()
                .is_some_and(|range| range.priority() > scan_range.priority());

            if scan_ranges_updated {
                break;
            }
        }
    }
}

/// Fetches information that the wallet requests to complete its view of transaction
/// history.
#[tracing::instrument(skip_all)]
async fn data_requests(
    chain: Chain,
    params: &Network,
    db_data: &mut DbConnection,
    tip_change_signal: Arc<Notify>,
) -> Result<(), SyncError> {
    loop {
        // Wait for the chain tip to advance
        tip_change_signal.notified().await;

        let chain_view = chain.snapshot();

        let requests = db_data.transaction_data_requests()?;
        if requests.is_empty() {
            // Wait for new requests.
            debug!("No transaction data requests, sleeping until the chain tip changes.");
            continue;
        }

        info!("{} transaction data requests to service", requests.len());
        for request in requests {
            match request {
                TransactionDataRequest::GetStatus(txid) => {
                    if txid.is_null() {
                        continue;
                    }

                    info!("Getting status of {txid}");
                    let status = chain_view
                        .get_transaction_status(txid)
                        .await
                        .map_err(SyncError::Chain)?;

                    db_data.set_transaction_status(txid, status)?;
                }
                TransactionDataRequest::Enhancement(txid) => {
                    if txid.is_null() {
                        continue;
                    }

                    info!("Enhancing {txid}");
                    if let Some(tx) = chain_view
                        .get_transaction(txid)
                        .await
                        .map_err(SyncError::Chain)?
                    {
                        decrypt_and_store_transaction(params, db_data, &tx.inner, tx.mined_height)?;
                    } else {
                        db_data
                            .set_transaction_status(txid, TransactionStatus::TxidNotRecognized)?;
                    }
                }
                // Ignore these, we do all transparent detection through full blocks.
                TransactionDataRequest::TransactionsInvolvingAddress(_) => (),
            }
        }
    }
}

/// Processes the queue of transactions that need to be scanned with the wallet's viewing
/// keys.
#[tracing::instrument(skip_all)]
async fn batch_decryptor(
    params: Network,
    db_data: &mut DbConnection,
    decryptor: decryptor::Engine<AccountUuid, (AccountUuid, Scope)>,
) -> Result<(), SyncError> {
    decryptor
        .run(params, || {
            // Fetch the UnifiedFullViewingKeys we are tracking.
            let account_ufvks = db_data.get_unified_full_viewing_keys()?;
            let scanning_keys = ScanningKeys::from_account_ufvks(account_ufvks);
            Ok::<_, SyncError>(scanning_keys)
        })
        .await
}
