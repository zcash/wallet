use documented::Documented;
use jsonrpsee::core::RpcResult;
use rand::rngs::OsRng;
use schemars::JsonSchema;
use serde::Serialize;
use zaino_state::{FetchServiceSubscriber, LightWalletIndexer};
use zcash_client_backend::data_api::{
    BlockMetadata, WalletRead, WalletSummary, scanning::ScanRange, wallet::ConfirmationsPolicy,
};
use zcash_client_sqlite::{AccountUuid, WalletDb, error::SqliteClientError, util::SystemClock};
use zcash_primitives::block::BlockHash;

use crate::{
    components::{database::DbConnection, json_rpc::server::LegacyCode},
    network::Network,
};

/// Response to a `getwalletstatus` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = GetWalletStatus;

/// The wallet status information.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct GetWalletStatus {
    /// The backing full node's view of the chain tip.
    node_tip: ChainTip,

    /// The wallet's view of the chain tip.
    ///
    /// This should only diverge from `node_tip` for very short periods of time.
    ///
    /// Omitted if the wallet has just been started for the first time and has not yet
    /// begun syncing.
    #[serde(skip_serializing_if = "Option::is_none")]
    wallet_tip: Option<ChainTip>,

    /// The height to which the wallet is fully synced.
    ///
    /// The wallet only has a partial view of chain data above this height.
    ///
    /// Omitted if the wallet does not have any accounts or birthday data and thus has
    /// nowhere to sync from, or if the wallet birthday itself has not yet been synced.
    /// The latter occurs when a recovered wallet first starts and is scanning the chain
    /// tip region.
    #[serde(skip_serializing_if = "Option::is_none")]
    fully_synced_height: Option<u32>,

    /// The height up to which the wallet has full information.
    ///
    /// Omitted if the wallet is fully synced (which requires that `fully_synced_height`
    /// is equal to `wallet_tip.height`).
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_work_remaining: Option<SyncWorkRemaining>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct ChainTip {
    /// The hash of the block at the chain tip.
    blockhash: String,

    /// The height of the block in the chain.
    height: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct SyncWorkRemaining {
    unscanned_blocks: u32,

    // TODO: Replace these with accurate unscanned note counts, which we can determine
    // because Zallet tracks the chain tip very closely.
    progress_numerator: u64,
    progress_denominator: u64,
}

pub(crate) async fn call(wallet: &DbConnection, chain: FetchServiceSubscriber) -> Response {
    let node_tip = chain
        .get_latest_block()
        .await
        // TODO: Better error.
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    let wallet_data = wallet
        .with(status_data)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    Ok(GetWalletStatus {
        node_tip: ChainTip {
            blockhash: BlockHash::try_from_slice(node_tip.hash.as_slice())
                .expect("block hash missing")
                .to_string(),
            height: u32::try_from(node_tip.height)
                // TODO: Better error.
                .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?,
        },
        wallet_tip: wallet_data.as_ref().map(|d| d.chain_tip()),
        fully_synced_height: wallet_data.as_ref().and_then(|d| d.fully_synced_height()),
        sync_work_remaining: wallet_data.as_ref().and_then(|d| d.sync_work_remaining()),
    })
}

/// Fetches status data from the wallet.
fn status_data(
    wallet: WalletDb<&rusqlite::Connection, Network, SystemClock, OsRng>,
) -> Result<Option<WalletData>, SqliteClientError> {
    let tip_height = wallet.chain_height()?;
    let tip_metadata = if let Some(block_height) = tip_height {
        wallet.block_metadata(block_height)?
    } else {
        None
    };

    if let Some(tip_metadata) = tip_metadata {
        let block_fully_scanned = wallet.block_fully_scanned()?;
        let scan_ranges = wallet.suggest_scan_ranges()?;
        let summary = wallet.get_wallet_summary(ConfirmationsPolicy::MIN)?;

        Ok(Some(WalletData {
            tip_metadata,
            block_fully_scanned,
            scan_ranges,
            summary,
        }))
    } else {
        Ok(None)
    }
}

struct WalletData {
    tip_metadata: BlockMetadata,
    block_fully_scanned: Option<BlockMetadata>,
    scan_ranges: Vec<ScanRange>,
    summary: Option<WalletSummary<AccountUuid>>,
}

impl WalletData {
    fn chain_tip(&self) -> ChainTip {
        ChainTip {
            blockhash: self.tip_metadata.block_hash().to_string(),
            height: self.tip_metadata.block_height().into(),
        }
    }

    fn fully_synced_height(&self) -> Option<u32> {
        self.block_fully_scanned.map(|b| b.block_height().into())
    }

    fn sync_work_remaining(&self) -> Option<SyncWorkRemaining> {
        self.summary.as_ref().and_then(|s| {
            let unscanned_blocks = self
                .scan_ranges
                .iter()
                .map(|r| r.block_range().end - r.block_range().start)
                .sum::<u32>();

            let (progress_numerator, progress_denominator) =
                if let Some(recovery) = s.progress().recovery() {
                    (
                        s.progress().scan().numerator() + recovery.numerator(),
                        s.progress().scan().denominator() + recovery.denominator(),
                    )
                } else {
                    (
                        *s.progress().scan().numerator(),
                        *s.progress().scan().denominator(),
                    )
                };

            if unscanned_blocks == 0 && progress_numerator == progress_denominator {
                None
            } else {
                Some(SyncWorkRemaining {
                    unscanned_blocks,
                    progress_numerator,
                    progress_denominator,
                })
            }
        })
    }
}
