use jsonrpsee::{
    core::RpcResult,
    types::{ErrorCode as RpcErrorCode, ErrorObjectOwned as RpcError},
};
use serde::Serialize;
use zcash_client_backend::data_api::WalletRead;
use zcash_protocol::{consensus::BlockHeight, TxId};

use crate::components::{database::DbConnection, json_rpc::server::LegacyCode};

/// Response to a `gettransaction` RPC request.
pub(crate) type Response = RpcResult<Transaction>;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Transaction {
    /// The transaction ID.
    txid: String,

    /// The transaction status.
    ///
    /// One of 'mined', 'waiting', 'expiringsoon' or 'expired'.
    status: &'static str,

    /// The transaction version.
    version: String,

    /// The transaction amount in ZEC.
    amount: f64,

    /// The amount in zatoshis.
    #[serde(rename = "amountZat")]
    amount_zat: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    fee: Option<u64>,

    /// The number of confirmations.
    ///
    /// - A positive value is the number of blocks that have been mined including the
    ///   transaction in the chain. For example, 1 confirmation means the transaction is
    ///   in the block currently at the chain tip.
    /// - 0 means the transaction is in the mempool. If `asOfHeight` was set, this case
    ///   will not occur.
    /// - -1 means the transaction cannot be mined.
    confirmations: i32,

    #[serde(skip_serializing_if = "Option::is_none")]
    generated: Option<bool>,

    /// The block hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    blockhash: Option<String>,

    /// The block index.
    #[serde(skip_serializing_if = "Option::is_none")]
    blockindex: Option<u16>,

    /// The time in seconds since epoch (1 Jan 1970 GMT).
    #[serde(skip_serializing_if = "Option::is_none")]
    blocktime: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    expiryheight: Option<u64>,

    walletconflicts: Vec<String>,

    /// The transaction time in seconds since epoch (1 Jan 1970 GMT).
    time: u64,

    /// The time received in seconds since epoch (1 Jan 1970 GMT).
    timereceived: u64,

    details: Vec<Detail>,

    /// Raw data for transaction.
    hex: String,
}

#[derive(Clone, Debug, Serialize)]
struct Detail {
    /// The Zcash address involved in the transaction.
    address: String,

    /// The category.
    ///
    /// One of 'send' or 'receive'.
    category: String,

    /// The amount in ZEC.
    amount: f64,

    /// The amount in zatoshis.
    #[serde(rename = "amountZat")]
    amount_zat: u64,

    /// The vout value.
    vout: u64,
}

pub(crate) fn call(
    wallet: &DbConnection,
    txid_str: &str,
    include_watchonly: bool,
    verbose: bool,
    as_of_height: i64,
) -> Response {
    let txid: TxId = txid_str.parse()?;

    if verbose {
        return Err(LegacyCode::InvalidParameter.with_static("verbose must be set to false"));
    }

    let as_of_height = match as_of_height {
        // The default, do nothing.
        -1 => Ok(None),
        ..0 => Err(LegacyCode::InvalidParameter
            .with_static("Can not perform the query as of a negative block height")),
        0 => Err(LegacyCode::InvalidParameter
            .with_static("Can not perform the query as of the genesis block")),
        1.. => u32::try_from(as_of_height).map(Some).map_err(|_| {
            LegacyCode::InvalidParameter.with_static("asOfHeight parameter is too big")
        }),
    }?;

    let tx = wallet
        .get_transaction(txid)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or(LegacyCode::InvalidParameter.with_static("Invalid or non-wallet transaction id"))?;

    let chain_height = wallet
        .chain_height()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or_else(|| LegacyCode::InWarmup.into())?;

    let mined_height = wallet
        .get_tx_height(txid)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    let confirmations = {
        let effective_chain_height = as_of_height
            .map(BlockHeight::from_u32)
            .unwrap_or(chain_height)
            .min(chain_height);
        match mined_height {
            Some(mined_height) => (effective_chain_height + 1 - mined_height) as i32,
            None => {
                // TODO: Also check if the transaction is in the mempool for this branch.
                if as_of_height.is_some() {
                    -1
                } else {
                    0
                }
            }
        }
    };

    let generated = if tx
        .transparent_bundle()
        .is_some_and(|bundle| bundle.is_coinbase())
    {
        Some(true)
    } else {
        None
    };

    let mut status = "waiting";

    let (blockhash, blockindex, blocktime, expiryheight) = if let Some(height) = mined_height {
        status = "mined";
        let block = wallet
            .block_metadata(height)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            // This would be a race condition between this and a reorg.
            .ok_or(RpcErrorCode::InternalError)?;
        (
            Some(block.block_hash().to_string()),
            None, // TODO: Some(wtx.nIndex),
            None, // TODO: Some(mapBlockIndex[wtx.hashBlock].GetBlockTime()),
            Some(tx.expiry_height().into()),
        )
    } else {
        match (
            is_expired_tx(&tx, chain_height),
            is_expiring_soon_tx(&tx, chain_height + 1),
        ) {
            (false, true) => status = "expiringsoon",
            (true, _) => status = "expired",
            _ => (),
        }
        (None, None, None, None)
    };

    let walletconflicts = vec![];

    let details = vec![];

    let hex_tx = {
        let mut bytes = vec![];
        tx.write(&mut bytes).expect("can write to Vec");
        hex::encode(bytes)
    };

    Ok(Transaction {
        txid: txid_str.into(),
        status,
        version: tx.version().header() & 0x7FFFFFFF,
        amount: (),
        amount_zat: (),
        fee: None,
        confirmations,
        generated,
        blockhash,
        blockindex,
        blocktime,
        expiryheight,
        walletconflicts,
        time: (),
        timereceived: (),
        details,
        hex: hex_tx,
    })
}

/// The number of blocks within expiry height when a tx is considered to be expiring soon.
const TX_EXPIRING_SOON_THRESHOLD: u32 = 3;

fn is_expired_tx(tx: &zcash_primitives::transaction::Transaction, height: BlockHeight) -> bool {
    if tx.expiry_height() == 0.into()
        || tx
            .transparent_bundle()
            .is_some_and(|bundle| bundle.is_coinbase())
    {
        false
    } else {
        height > tx.expiry_height()
    }
}

fn is_expiring_soon_tx(
    tx: &zcash_primitives::transaction::Transaction,
    next_height: BlockHeight,
) -> bool {
    is_expired_tx(tx, next_height + TX_EXPIRING_SOON_THRESHOLD)
}
