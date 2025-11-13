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

#![allow(deprecated)] // For zaino

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::Duration;

use futures::{StreamExt as _, TryStreamExt as _};
use jsonrpsee::tracing::{self, debug, info};
use tokio::{sync::Notify, time};
use transparent::{
    address::Script,
    bundle::{OutPoint, TxOut},
};
use zaino_proto::proto::service::GetAddressUtxosArg;
use zaino_state::{ChainIndex, LightWalletIndexer as _, TransactionHash, ZcashIndexer};
use zcash_client_backend::{
    data_api::{
        OutputStatusFilter, TransactionDataRequest, TransactionStatus, TransactionStatusFilter,
        WalletRead, WalletWrite,
        chain::{self, BlockCache, scan_cached_blocks},
        scanning::{ScanPriority, ScanRange},
        wallet::decrypt_and_store_transaction,
    },
    scanning::ScanError,
    wallet::WalletTransparentOutput,
};
use zcash_encoding::Vector;
use zcash_keys::encoding::AddressCodec;
use zcash_primitives::{
    block::{BlockHash, BlockHeader},
    transaction::Transaction,
};
use zcash_protocol::{
    TxId,
    consensus::{self, BlockHeight},
    value::Zatoshis,
};
use zcash_script::script;
use zebra_rpc::methods::{AddressStrings, GetAddressTxIdsRequest};

use super::{
    TaskHandle,
    chain::{Chain, ChainBlock},
    database::{Database, DbConnection},
};
use crate::{
    components::json_rpc::utils::parse_txid, config::ZalletConfig, error::Error, network::Network,
};

mod cache;

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
    ) -> Result<(TaskHandle, TaskHandle, /*TaskHandle,*/ TaskHandle), Error> {
        let params = config.consensus.network();

        // Ensure the wallet is in a state that the sync tasks can work with.
        let mut db_data = db.handle().await?;
        let (starting_tip, starting_boundary) =
            initialize(&chain, &params, db_data.as_mut()).await?;

        // Manage the boundary between the `steady_state` and `recover_history` tasks with
        // an atomic.
        let current_boundary = Arc::new(AtomicU32::new(starting_boundary.into()));

        // TODO: Zaino should provide us an API that allows us to be notified when the chain tip
        // changes; here, we produce our own signal via the "mempool stream closing" side effect
        // that occurs in the light client API when the chain tip changes.
        let tip_change_signal_source = Arc::new(Notify::new());
        // let poll_tip_change_signal_receiver = tip_change_signal_source.clone();
        let req_tip_change_signal_receiver = tip_change_signal_source.clone();

        // Spawn the ongoing sync tasks.
        let steady_state_task = {
            let chain = chain.clone();
            let lower_boundary = current_boundary.clone();
            crate::spawn!("Steady state sync", async move {
                steady_state(
                    chain,
                    &params,
                    db_data.as_mut(),
                    starting_tip,
                    lower_boundary,
                    tip_change_signal_source,
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
                recover_history(chain, &params, db_data.as_mut(), upper_boundary, 1000).await?;
                Ok(())
            })
        };

        // let mut db_data = db.handle().await?;
        // let poll_transparent_task = crate::spawn!("Poll transparent", async move {
        //     poll_transparent(
        //         chain,
        //         &params,
        //         db_data.as_mut(),
        //         poll_tip_change_signal_receiver,
        //     )
        //     .await?;
        //     Ok(())
        // });

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
            steady_state_task,
            recover_history_task,
            // poll_transparent_task,
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
        let current_tip = chain_view.tip();
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

        // Fetch blocks that need to be applied to the wallet.
        let blocks_to_apply = chain_view.stream_blocks(scan_range.block_range());
        tokio::pin!(blocks_to_apply);

        // TODO: Load this into the wallet DB.
        let from_state = chain_view
            .tree_state_as_of(scan_range.block_range().start - 1)
            .await
            .map_err(SyncError::Indexer)?;

        while let Some((height, block)) = blocks_to_apply
            .try_next()
            .await
            .map_err(SyncError::Indexer)?
        {
            steps::scan_block(db_data, params, height, block).await?;
        }
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
) -> Result<(), SyncError> {
    info!("Steady-state sync task started");

    loop {
        let chain_view = chain.snapshot();
        let current_tip = chain_view.tip();

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
            .map_err(SyncError::Indexer)?
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
        while let Some((height, block)) = blocks_to_apply
            .try_next()
            .await
            .map_err(SyncError::Indexer)?
        {
            assert_eq!(height, prev_tip.height + 1);
            let current_tip = ChainBlock {
                height,
                hash: block.header.hash(),
            };
            db_data.update_chain_tip(height)?;

            steps::scan_block(db_data, params, height, block).await?;

            // Now that we're done applying the block, update our chain pointer.
            prev_tip = current_tip;
        }

        // If we have caught up to the chain tip, stream the mempool state into the wallet.
        if let Some(mempool_stream) = chain_view.get_mempool_stream() {
            info!("Reached chain tip, streaming mempool");
            tokio::pin!(mempool_stream);
            while let Some(tx) = mempool_stream.next().await {
                info!("Scanning mempool tx {}", tx.txid());
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
            let chain_view = chain.snapshot();
            let blocks_to_apply = chain_view.stream_blocks(scan_range.block_range());
            tokio::pin!(blocks_to_apply);

            let from_state = chain_view
                .tree_state_as_of(scan_range.block_range().start - 1)
                .await
                .map_err(SyncError::Indexer)?;

            // Scan the downloaded blocks.
            while let Some((height, block)) = blocks_to_apply
                .try_next()
                .await
                .map_err(SyncError::Indexer)?
            {
                steps::scan_block(db_data, params, height, block).await?;
            }

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

// /// Polls the non-ephemeral transparent addresses in the wallet for UTXOs.
// ///
// /// Ephemeral addresses are handled by [`data_requests`].
// #[tracing::instrument(skip_all)]
// async fn poll_transparent(
//     chain: Chain,
//     params: &Network,
//     db_data: &mut DbConnection,
//     tip_change_signal: Arc<Notify>,
// ) -> Result<(), SyncError> {
//     info!("Transparent address polling sync task started");

//     loop {
//         // Wait for the chain tip to advance
//         tip_change_signal.notified().await;

//         // Collect all of the wallet's non-ephemeral transparent addresses. We do this
//         // fresh every loop to ensure we incorporate changes to the address set.
//         //
//         // TODO: This is likely to be append-only unless we add support for removing an
//         // account from the wallet, so we could implement a more efficient strategy here
//         // with some changes to the `WalletRead` API. For now this is fine.
//         let addresses = db_data
//             .get_account_ids()?
//             .into_iter()
//             .map(|account| db_data.get_transparent_receivers(account, true, true))
//             .collect::<Result<Vec<_>, _>>()?
//             .into_iter()
//             .flat_map(|m| m.into_keys().map(|addr| addr.encode(params)))
//             .collect();

//         // Fetch all mined UTXOs.
//         // TODO: I really want to use the chaininfo-aware version (which Zaino doesn't
//         // implement) or an equivalent Zaino index (once it exists).
//         info!("Fetching mined UTXOs");
//         let utxos = chain
//             .z_get_address_utxos(AddressStrings::new(addresses))
//             .await?;

//         // Notify the wallet about all mined UTXOs.
//         for utxo in utxos {
//             let (address, txid, index, script, value_zat, mined_height) = utxo.into_parts();
//             debug!("{address} has UTXO in tx {txid} at index {}", index.index());

//             let output = WalletTransparentOutput::from_parts(
//                 OutPoint::new(txid.0, index.index()),
//                 TxOut::new(
//                     Zatoshis::const_from_u64(value_zat),
//                     Script(script::Code(script.as_raw_bytes().to_vec())),
//                 ),
//                 Some(BlockHeight::from_u32(mined_height.0)),
//             )
//             .expect("the UTXO was detected via a supported address kind");

//             db_data.put_received_transparent_utxo(&output)?;
//         }
//         // TODO: Once Zaino has an index over the mempool, monitor it for changes to the
//         // unmined UTXO set (which we can't get directly from the stream without building
//         // an index because existing mempool txs can be spent within the mempool).
//     }
// }

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
                        .map_err(SyncError::Indexer)?;

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
                        .map_err(SyncError::Indexer)?
                    {
                        decrypt_and_store_transaction(params, db_data, &tx.inner, tx.mined_height)?;
                    } else {
                        db_data
                            .set_transaction_status(txid, TransactionStatus::TxidNotRecognized)?;
                    }
                }
                // Ignore these, we do all transparent detection through full blocks.
                TransactionDataRequest::TransactionsInvolvingAddress(_) => (),
                // TransactionDataRequest::TransactionsInvolvingAddress(req) => {
                //     // nb: Zallet is a full node wallet with an index; we can safely look up
                //     // information immediately without exposing correlations between addresses to
                //     // untrusted parties, so we can ignore the `request_at` field.

                //     // TODO: we're making the *large* assumption that the chain data doesn't update
                //     // between the multiple chain calls in this method. Ideally, Zaino will give us
                //     // a "transactional" API so that we can ensure internal consistency; for now,
                //     // we pick the chain height as of the start of evaluation as the "evaluated-at"
                //     // height for this data request, in order to not overstate the height for which
                //     // all observations are valid.
                //     // TODO: Now that we have a transactional ChainView, rework this logic.
                //     let as_of_height = match req.block_range_end() {
                //         Some(h) => h - 1,
                //         None => chain_view.tip().height,
                //     };

                //     let address = req.address().encode(params);
                //     debug!(
                //         tx_status_filter = ?req.tx_status_filter(),
                //         output_status_filter = ?req.output_status_filter(),
                //         "Fetching transactions involving {address} in range {}..{}",
                //         req.block_range_start(),
                //         req.block_range_end().map(|h| h.to_string()).unwrap_or_default(),
                //     );

                //     let request = GetAddressTxIdsRequest::new(
                //         vec![address.clone()],
                //         Some(u32::from(req.block_range_start())),
                //         req.block_range_end().map(u32::from),
                //     );

                //     // Zallet is a full node wallet with an index; we can safely look up
                //     // information immediately without exposing correlations between
                //     // addresses to untrusted parties, so we ignore `req.request_at().

                //     let txs_with_unspent_outputs = match req.output_status_filter() {
                //         OutputStatusFilter::Unspent => {
                //             let request = GetAddressUtxosArg {
                //                 addresses: vec![address],
                //                 start_height: req.block_range_start().into(),
                //                 max_entries: 0,
                //             };
                //             Some(
                //                 chain
                //                     .get_address_utxos(request)
                //                     .await.map_err(SyncError::Indexer)?
                //                     .address_utxos
                //                     .into_iter()
                //                     .map(|utxo| {
                //                         TxId::read(utxo.txid.as_slice())
                //                             .expect("TODO: Zaino's API should have caught this error for us")
                //                     })
                //                     .collect::<HashSet<_>>(),
                //             )
                //         }
                //         OutputStatusFilter::All => None,
                //     };

                //     for txid_str in chain.get_address_tx_ids(request).await? {
                //         let txid = parse_txid(&txid_str)
                //             .expect("TODO: Zaino's API should have caught this error for us");

                //         let tx = match chain.get_raw_transaction(txid_str, Some(1)).await? {
                //             // TODO: Zaino should have a Rust API for fetching tx details,
                //             // instead of requiring us to specify a verbosity and then deal
                //             // with an enum variant that should never occur.
                //             zebra_rpc::methods::GetRawTransaction::Raw(_) => unreachable!(),
                //             zebra_rpc::methods::GetRawTransaction::Object(tx) => tx,
                //         };

                //         // Ignore transactions that don't exist in the main chain or its mempool.
                //         let mined_height = match tx.height() {
                //             None => None,
                //             Some(h @ 0..) => Some(BlockHeight::from_u32(h as u32)),
                //             Some(_) => continue,
                //         };

                //         // Ignore transactions that don't match the status filter.
                //         match (&req.tx_status_filter(), mined_height) {
                //             (TransactionStatusFilter::Mined, None)
                //             | (TransactionStatusFilter::Mempool, Some(_)) => continue,
                //             _ => (),
                //         }

                //         // Ignore transactions with outputs that don't match the status
                //         // filter.
                //         if let Some(filter) = &txs_with_unspent_outputs {
                //             if !filter.contains(&txid) {
                //                 continue;
                //             }
                //         }

                //         // TODO: Zaino should either be doing the tx parsing for us,
                //         // or telling us the consensus branch ID for which the tx is
                //         // treated as valid.
                //         // TODO: We should make the consensus branch ID optional in
                //         // the parser, so Zaino only need to provide it to us for v4
                //         // or earlier txs.
                //         let parse_height = match mined_height {
                //             Some(height) => height,
                //             None => {
                //                 let chain_height = BlockHeight::from_u32(
                //                     // TODO: Zaino should be returning this as a u32, or
                //                     // ideally as a `BlockHeight`.
                //                     chain.get_latest_block().await?.height.try_into().expect(
                //                         "TODO: Zaino's API should have caught this error for us",
                //                     ),
                //                 );
                //                 // If the transaction is not mined, it is validated at the
                //                 // "mempool height" which is the height that the next
                //                 // mined block would have.
                //                 chain_height + 1
                //             }
                //         };
                //         let tx = Transaction::read(
                //             tx.hex().as_ref(),
                //             consensus::BranchId::for_height(params, parse_height),
                //         )
                //         .expect("TODO: Zaino's API should have caught this error for us");

                //         decrypt_and_store_transaction(params, db_data, &tx, mined_height)?;
                //     }

                //     db_data.notify_address_checked(req, as_of_height)?;
                // }
            }
        }
    }
}
