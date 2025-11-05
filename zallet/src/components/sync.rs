use std::{collections::HashSet, sync::Arc, time::Duration};

use futures::StreamExt as _;
use jsonrpsee::tracing::{self, debug, info, warn};
use tokio::{sync::Notify, time};
use transparent::{
    address::Script,
    bundle::{OutPoint, TxOut},
};
use zaino_proto::proto::service::GetAddressUtxosArg;
use zaino_state::{
    FetchServiceError, FetchServiceSubscriber, LightWalletIndexer as _, ZcashIndexer,
};
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
use zcash_keys::encoding::AddressCodec;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    TxId,
    consensus::{self, BlockHeight},
    value::Zatoshis,
};
use zcash_script::script;
use zebra_chain::transaction::SerializedTransaction;
use zebra_rpc::methods::{AddressStrings, GetAddressTxIdsRequest};

use super::{
    TaskHandle,
    chain_view::ChainView,
    database::{Database, DbConnection},
};
use crate::{
    components::json_rpc::utils::parse_txid, config::ZalletConfig, error::Error, network::Network,
};

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
    ) -> Result<(TaskHandle, TaskHandle, TaskHandle, TaskHandle), Error> {
        let params = config.consensus.network();

        // Ensure the wallet is in a state that the sync tasks can work with.
        let chain = chain_view.subscribe().await?.inner();
        let mut db_data = db.handle().await?;
        let starting_tip = initialize(chain, &params, db_data.as_mut()).await?;
        // TODO: Zaino should provide us an API that allows us to be notified when the chain tip
        // changes; here, we produce our own signal via the "mempool stream closing" side effect
        // that occurs in the light client API when the chain tip changes.
        let tip_change_signal_source = Arc::new(Notify::new());
        let poll_tip_change_signal_receiver = tip_change_signal_source.clone();
        let req_tip_change_signal_receiver = tip_change_signal_source.clone();

        // Spawn the ongoing sync tasks.
        let chain = chain_view.subscribe().await?.inner();
        let steady_state_task = crate::spawn!("Steady state sync", async move {
            steady_state(
                &chain,
                &params,
                db_data.as_mut(),
                starting_tip,
                tip_change_signal_source,
            )
            .await?;
            Ok(())
        });

        let chain = chain_view.subscribe().await?.inner();
        let mut db_data = db.handle().await?;
        let recover_history_task = crate::spawn!("Recover history", async move {
            recover_history(chain, &params, db_data.as_mut(), 1000).await?;
            Ok(())
        });

        let chain = chain_view.subscribe().await?.inner();
        let mut db_data = db.handle().await?;
        let poll_transparent_task = crate::spawn!("Poll transparent", async move {
            poll_transparent(
                chain,
                &params,
                db_data.as_mut(),
                poll_tip_change_signal_receiver,
            )
            .await?;
            Ok(())
        });

        let chain = chain_view.subscribe().await?.inner();
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
            poll_transparent_task,
            data_requests_task,
        ))
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
            steps::fetch_chain_state(&chain, params, scan_range.block_range().start - 1).await?;

        // Scan the downloaded blocks.
        tokio::task::block_in_place(|| {
            info!("Scanning {}", scan_range);
            match scan_cached_blocks(
                params,
                &db_cache,
                db_data,
                scan_range.block_range().start,
                &from_state,
                scan_range.len(),
            ) {
                Ok(_) => Ok(()),
                Err(chain::error::Error::Scan(ScanError::PrevHashMismatch { at_height })) => {
                    db_data
                        .truncate_to_height(at_height - 10)
                        .map_err(chain::error::Error::Wallet)?;
                    Ok(())
                }
                Err(e) => Err(e),
            }
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
    tip_change_signal: Arc<Notify>,
) -> Result<(), SyncError> {
    info!("Steady-state sync task started");
    let mut current_tip = steps::get_chain_tip(chain).await?;

    // TODO: Remove this once we've made `zcash_client_sqlite` changes to support scanning
    // regular blocks.
    let db_cache = cache::MemoryCache::new();

    loop {
        info!("New chain tip: {} {}", current_tip.height, current_tip.hash);
        tip_change_signal.notify_one();

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

            let from_state =
                steps::fetch_chain_state(chain, params, from_height.saturating_sub(1)).await?;

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
                steps::fetch_chain_state(&chain, params, scan_range.block_range().start - 1)
                    .await?;

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

/// Polls the non-ephemeral transparent addresses in the wallet for UTXOs.
///
/// Ephemeral addresses are handled by [`data_requests`].
#[tracing::instrument(skip_all)]
async fn poll_transparent(
    chain: FetchServiceSubscriber,
    params: &Network,
    db_data: &mut DbConnection,
    tip_change_signal: Arc<Notify>,
) -> Result<(), SyncError> {
    info!("Transparent address polling sync task started");

    loop {
        // Wait for the chain tip to advance
        tip_change_signal.notified().await;

        // Collect all of the wallet's non-ephemeral transparent addresses. We do this
        // fresh every loop to ensure we incorporate changes to the address set.
        //
        // TODO: This is likely to be append-only unless we add support for removing an
        // account from the wallet, so we could implement a more efficient strategy here
        // with some changes to the `WalletRead` API. For now this is fine.
        let addresses = db_data
            .get_account_ids()?
            .into_iter()
            .map(|account| db_data.get_transparent_receivers(account, true, true))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flat_map(|m| m.into_keys().map(|addr| addr.encode(params)))
            .collect();

        // Fetch all mined UTXOs.
        // TODO: I really want to use the chaininfo-aware version (which Zaino doesn't
        // implement) or an equivalent Zaino index (once it exists).
        info!("Fetching mined UTXOs");
        let utxos = chain
            .z_get_address_utxos(AddressStrings::new(addresses))
            .await?;

        // Notify the wallet about all mined UTXOs.
        for utxo in utxos {
            let (address, txid, index, script, value_zat, mined_height) = utxo.into_parts();
            debug!("{address} has UTXO in tx {txid} at index {}", index.index());

            let output = WalletTransparentOutput::from_parts(
                OutPoint::new(txid.0, index.index()),
                TxOut::new(
                    Zatoshis::const_from_u64(value_zat),
                    Script(script::Code(script.as_raw_bytes().to_vec())),
                ),
                Some(BlockHeight::from_u32(mined_height.0)),
            )
            .expect("the UTXO was detected via a supported address kind");

            db_data.put_received_transparent_utxo(&output)?;
        }
        // TODO: Once Zaino has an index over the mempool, monitor it for changes to the
        // unmined UTXO set (which we can't get directly from the stream without building
        // an index because existing mempool txs can be spent within the mempool).
    }
}

/// Fetches information that the wallet requests to complete its view of transaction
/// history.
#[tracing::instrument(skip_all)]
async fn data_requests(
    chain: FetchServiceSubscriber,
    params: &Network,
    db_data: &mut DbConnection,
    tip_change_signal: Arc<Notify>,
) -> Result<(), SyncError> {
    loop {
        // Wait for the chain tip to advance
        tip_change_signal.notified().await;

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
                    let status = match chain.get_raw_transaction(txid.to_string(), Some(1)).await {
                        // TODO: Zaino should have a Rust API for fetching tx details,
                        // instead of requiring us to specify a verbosity and then deal
                        // with an enum variant that should never occur.
                        Ok(zebra_rpc::methods::GetRawTransaction::Raw(_)) => unreachable!(),
                        Ok(zebra_rpc::methods::GetRawTransaction::Object(tx)) => tx
                            .height()
                            .map(BlockHeight::from_u32)
                            .map(TransactionStatus::Mined)
                            .unwrap_or(TransactionStatus::NotInMainChain),
                        // TODO: Zaino is not correctly parsing the error response, so we
                        // can't look for `LegacyCode::InvalidAddressOrKey`. Instead match
                        // on these three possible error messages:
                        // - "No such mempool or blockchain transaction" (zcashd -txindex)
                        // - "No such mempool transaction." (zcashd)
                        // - "No such mempool or main chain transaction" (zebrad)
                        Err(FetchServiceError::RpcError(e))
                            if e.message.contains("No such mempool") =>
                        {
                            TransactionStatus::TxidNotRecognized
                        }
                        Err(e) => return Err(e.into()),
                    };

                    db_data.set_transaction_status(txid, status)?;
                }
                TransactionDataRequest::Enhancement(txid) => {
                    if txid.is_null() {
                        continue;
                    }

                    info!("Enhancing {txid}");
                    let tx = match chain.get_raw_transaction(txid.to_string(), Some(1)).await {
                        // TODO: Zaino should have a Rust API for fetching tx details,
                        // instead of requiring us to specify a verbosity and then deal
                        // with an enum variant that should never occur.
                        Ok(zebra_rpc::methods::GetRawTransaction::Raw(_)) => unreachable!(),
                        Ok(zebra_rpc::methods::GetRawTransaction::Object(tx)) => {
                            let mined_height = tx.height().map(BlockHeight::from_u32);

                            // TODO: Zaino should either be doing the tx parsing for us,
                            // or telling us the consensus branch ID for which the tx is
                            // treated as valid.
                            // TODO: We should make the consensus branch ID optional in
                            // the parser, so Zaino only need to provide it to us for v4
                            // or earlier txs.
                            let parse_height = match mined_height {
                                Some(height) => height,
                                None => {
                                    let chain_height = BlockHeight::from_u32(
                                        // TODO: Zaino should be returning this as a u32,
                                        // or ideally as a `BlockHeight`.
                                        chain.get_latest_block().await?.height.try_into().expect(
                                            "TODO: Zaino's API should have caught this error for us",
                                        ),
                                    );
                                    // If the transaction is not mined, it is validated at
                                    // the "mempool height" which is the height that the
                                    // next mined block would have.
                                    chain_height + 1
                                }
                            };
                            let tx = Transaction::read(
                                tx.hex().as_ref(),
                                consensus::BranchId::for_height(params, parse_height),
                            )
                            .expect("TODO: Zaino's API should have caught this error for us");

                            Some((tx, mined_height))
                        }
                        // TODO: Zaino is not correctly parsing the error response, so we
                        // can't look for `LegacyCode::InvalidAddressOrKey`. Instead match
                        // on these three possible error messages:
                        // - "No such mempool or blockchain transaction" (zcashd -txindex)
                        // - "No such mempool transaction." (zcashd)
                        // - "No such mempool or main chain transaction" (zebrad)
                        Err(FetchServiceError::RpcError(e))
                            if e.message.contains("No such mempool") =>
                        {
                            None
                        }
                        Err(e) => return Err(e.into()),
                    };

                    if let Some((tx, mined_height)) = tx {
                        decrypt_and_store_transaction(params, db_data, &tx, mined_height)?;
                    } else {
                        db_data
                            .set_transaction_status(txid, TransactionStatus::TxidNotRecognized)?;
                    }
                }
                TransactionDataRequest::TransactionsInvolvingAddress(req) => {
                    // nb: Zallet is a full node wallet with an index; we can safely look up
                    // information immediately without exposing correlations between addresses to
                    // untrusted parties, so we can ignore the `request_at` field.

                    // TODO: we're making the *large* assumption that the chain data doesn't update
                    // between the multiple chain calls in this method. Ideally, Zaino will give us
                    // a "transactional" API so that we can ensure internal consistency; for now,
                    // we pick the chain height as of the start of evaluation as the "evaluated-at"
                    // height for this data request, in order to not overstate the height for which
                    // all observations are valid.
                    let as_of_height = match req.block_range_end() {
                        Some(h) => h - 1,
                        None => chain.chain_height().await?.0.into(),
                    };

                    let address = req.address().encode(params);
                    debug!(
                        tx_status_filter = ?req.tx_status_filter(),
                        output_status_filter = ?req.output_status_filter(),
                        "Fetching transactions involving {address} in range {}..{}",
                        req.block_range_start(),
                        req.block_range_end().map(|h| h.to_string()).unwrap_or_default(),
                    );

                    let request = GetAddressTxIdsRequest::new(
                        vec![address.clone()],
                        Some(u32::from(req.block_range_start())),
                        req.block_range_end().map(u32::from),
                    );

                    // Zallet is a full node wallet with an index; we can safely look up
                    // information immediately without exposing correlations between
                    // addresses to untrusted parties, so we ignore `req.request_at().

                    let txs_with_unspent_outputs = match req.output_status_filter() {
                        OutputStatusFilter::Unspent => {
                            let request = GetAddressUtxosArg {
                                addresses: vec![address],
                                start_height: req.block_range_start().into(),
                                max_entries: 0,
                            };
                            Some(
                                chain
                                    .get_address_utxos(request)
                                    .await?
                                    .address_utxos
                                    .into_iter()
                                    .map(|utxo| {
                                        TxId::read(utxo.txid.as_slice())
                                            .expect("TODO: Zaino's API should have caught this error for us")
                                    })
                                    .collect::<HashSet<_>>(),
                            )
                        }
                        OutputStatusFilter::All => None,
                    };

                    for txid_str in chain.get_address_tx_ids(request).await? {
                        let txid = parse_txid(&txid_str)
                            .expect("TODO: Zaino's API should have caught this error for us");

                        let tx = match chain.get_raw_transaction(txid_str, Some(1)).await? {
                            // TODO: Zaino should have a Rust API for fetching tx details,
                            // instead of requiring us to specify a verbosity and then deal
                            // with an enum variant that should never occur.
                            zebra_rpc::methods::GetRawTransaction::Raw(_) => unreachable!(),
                            zebra_rpc::methods::GetRawTransaction::Object(tx) => tx,
                        };

                        let mined_height = tx.height().map(BlockHeight::from_u32);

                        // Ignore transactions that don't match the status filter.
                        match (&req.tx_status_filter(), mined_height) {
                            (TransactionStatusFilter::Mined, None)
                            | (TransactionStatusFilter::Mempool, Some(_)) => continue,
                            _ => (),
                        }

                        // Ignore transactions with outputs that don't match the status
                        // filter.
                        if let Some(filter) = &txs_with_unspent_outputs {
                            if !filter.contains(&txid) {
                                continue;
                            }
                        }

                        // TODO: Zaino should either be doing the tx parsing for us,
                        // or telling us the consensus branch ID for which the tx is
                        // treated as valid.
                        // TODO: We should make the consensus branch ID optional in
                        // the parser, so Zaino only need to provide it to us for v4
                        // or earlier txs.
                        let parse_height = match mined_height {
                            Some(height) => height,
                            None => {
                                let chain_height = BlockHeight::from_u32(
                                    // TODO: Zaino should be returning this as a u32, or
                                    // ideally as a `BlockHeight`.
                                    chain.get_latest_block().await?.height.try_into().expect(
                                        "TODO: Zaino's API should have caught this error for us",
                                    ),
                                );
                                // If the transaction is not mined, it is validated at the
                                // "mempool height" which is the height that the next
                                // mined block would have.
                                chain_height + 1
                            }
                        };
                        let tx = Transaction::read(
                            tx.hex().as_ref(),
                            consensus::BranchId::for_height(params, parse_height),
                        )
                        .expect("TODO: Zaino's API should have caught this error for us");

                        decrypt_and_store_transaction(params, db_data, &tx, mined_height)?;
                    }

                    db_data.notify_address_checked(req, as_of_height)?;
                }
            }
        }
    }
}
