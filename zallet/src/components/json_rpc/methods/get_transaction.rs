use documented::Documented;
use jsonrpsee::{core::RpcResult, types::ErrorCode as RpcErrorCode};
use schemars::JsonSchema;
use serde::Serialize;
use zaino_proto::proto::service::BlockId;
use zaino_state::{FetchServiceSubscriber, LightWalletIndexer};
use zcash_client_backend::data_api::WalletRead;
use zcash_protocol::{
    consensus::BlockHeight,
    value::{ZatBalance, Zatoshis},
};

use crate::components::{
    database::DbConnection,
    json_rpc::{
        balance::{
            is_mine_spendable, is_mine_spendable_or_watchonly, wtx_get_credit, wtx_get_debit,
            wtx_get_value_out, wtx_is_from_me,
        },
        server::LegacyCode,
        utils::{JsonZecBalance, parse_as_of_height, parse_txid, value_from_zat_balance},
    },
};

/// Response to a `gettransaction` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = Transaction;

/// Detailed transparent information about an in-wallet transaction.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct Transaction {
    /// The transaction ID.
    txid: String,

    /// The transaction status.
    ///
    /// One of 'mined', 'waiting', 'expiringsoon' or 'expired'.
    status: &'static str,

    /// The transaction version.
    version: u32,

    /// The transaction amount in ZEC.
    amount: JsonZecBalance,

    /// The amount in zatoshis.
    #[serde(rename = "amountZat")]
    amount_zat: i64,

    // TODO: Fee field might be negative when shown
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

#[derive(Clone, Debug, Serialize, JsonSchema)]
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

pub(super) const PARAM_TXID_DESC: &str = "The ID of the transaction to view.";
pub(super) const PARAM_INCLUDE_WATCHONLY_DESC: &str =
    "Whether to include watchonly addresses in balance calculation and `details`.";
pub(super) const PARAM_VERBOSE_DESC: &str = "Must be `false` or omitted.";
pub(super) const PARAM_AS_OF_HEIGHT_DESC: &str = "Execute the query as if it were run when the blockchain was at the height specified by this argument.";

pub(crate) async fn call(
    wallet: &DbConnection,
    chain: FetchServiceSubscriber,
    txid_str: &str,
    include_watchonly: bool,
    verbose: bool,
    as_of_height: Option<i64>,
) -> Response {
    let txid = parse_txid(txid_str)?;

    let filter = if include_watchonly {
        is_mine_spendable_or_watchonly
    } else {
        is_mine_spendable
    };

    if verbose {
        return Err(LegacyCode::InvalidParameter.with_static("verbose must be set to false"));
    }

    let as_of_height = parse_as_of_height(as_of_height)?;

    // Fetch this early so we can detect if the wallet is not ready yet.
    let chain_height = wallet
        .chain_height()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or_else(|| LegacyCode::InWarmup.with_static("Wait for the wallet to start up"))?;

    let tx = wallet
        .get_transaction(txid)
        .map_err(|e| {
            LegacyCode::Database
                .with_message(format!("Failed to fetch transaction: {}", e.to_string()))
        })?
        .ok_or(LegacyCode::InvalidParameter.with_static("Invalid or non-wallet transaction id"))?;

    // TODO: In zcashd `filter` is for the entire transparent wallet. Here we have multiple
    // mnemonics; do we have multiple transparent buckets of funds?

    // `gettransaction` was never altered to take wallet shielded notes into account.
    // As such, its `amount` and `fee` fields are calculated as if the wallet only has
    // transparent addresses.
    let (amount, fee) = {
        let credit = wtx_get_credit(wallet, &chain, &tx, as_of_height, filter)
            .await
            .map_err(|e| {
                LegacyCode::Database
                    .with_message(format!("wtx_get_credit failed: {}", e.to_string()))
            })?
            .ok_or_else(|| {
                // TODO: Either ensure this matches zcashd, or pick something better.
                LegacyCode::Misc.with_static("CWallet::GetCredit(): value out of range")
            })?;

        let debit = wtx_get_debit(wallet, &tx, filter)
            .map_err(|e| {
                LegacyCode::Database
                    .with_message(format!("wtx_get_debit failed: {}", e.to_string()))
            })?
            .ok_or_else(|| {
                // TODO: Either ensure this matches zcashd, or pick something better.
                LegacyCode::Misc.with_static("CWallet::GetDebit(): value out of range")
            })?;

        // - For transparent receive, this is `received`
        // - For transparent spend, this is `change - spent`
        let net = (ZatBalance::from(credit) - ZatBalance::from(debit)).expect("cannot underflow");

        // TODO: Alter the semantics here to instead use the concrete fee (spends - outputs).
        // In particular, for v6 txs this should equal the fee field, and it wouldn't with zcashd semantics.
        // See also https://github.com/zcash/zcash/issues/6821
        let fee = if wtx_is_from_me(wallet, &tx, filter).map_err(|e| {
            // TODO: Either ensure this matches zcashd, or pick something better.
            LegacyCode::Misc.with_message(e.to_string())
        })? {
            // - For transparent receive, this would be `value_out`, but we don't expose fee in this case.
            // - For transparent spend, this is `value_out - spent`, which should be negative.
            Some(
                (wtx_get_value_out(&tx).ok_or_else(|| {
                    // TODO: Either ensure this matches zcashd, or pick something better.
                    LegacyCode::Misc.with_static("CTransaction::GetValueOut(): value out of range")
                })? - debit)
                    .expect("cannot underflow"),
            )
        } else {
            None
        };

        (
            // - For transparent receive, this is `received`
            // - For transparent spend, this is `(change - spent) - (value_out - spent) = change - value_out`.
            (net - fee.unwrap_or(Zatoshis::ZERO).into()).expect("cannot underflow"),
            fee.map(u64::from),
        )
    };

    // TODO: Either update `zcash_client_sqlite` to store the time a transaction was first
    // detected, or add a Zallet database for tracking Zallet-specific tx metadata.
    let timereceived = 0;

    //
    // Below here is equivalent to `WalletTxToJSON` in `zcashd`.
    //

    let mined_height = wallet.get_tx_height(txid).map_err(|e| {
        LegacyCode::Database.with_message(format!("get_tx_height failed: {}", e.to_string()))
    })?;

    let confirmations = {
        let effective_chain_height = as_of_height.unwrap_or(chain_height).min(chain_height);
        match mined_height {
            Some(mined_height) => (effective_chain_height + 1 - mined_height) as i32,
            None => {
                // TODO: Also check if the transaction is in the mempool for this branch.
                if as_of_height.is_some() { -1 } else { 0 }
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
        let block_metadata = wallet
            .block_metadata(height)
            .map_err(|e| {
                LegacyCode::Database
                    .with_message(format!("block_metadata failed: {}", e.to_string()))
            })?
            // This would be a race condition between this and a reorg.
            .ok_or(RpcErrorCode::InternalError)?;

        let block = chain
            .get_block(BlockId {
                height: 0,
                hash: block_metadata.block_hash().0.to_vec(),
            })
            .await
            // This would be a race condition between this and a reorg.
            // TODO: Once Zaino updates its API to support atomic queries, it should not
            // be possible to fail to fetch the block that a transaction was observed
            // mined in.
            .map_err(|e| {
                LegacyCode::Database.with_message(format!("get_block failed: {}", e.to_string()))
            })?;

        let tx_index = block
            .vtx
            .iter()
            .find(|tx| tx.hash == block_metadata.block_hash().0)
            .map(|tx| u16::try_from(tx.index).expect("Zaino should provide valid data"));

        (
            Some(block_metadata.block_hash().to_string()),
            tx_index,
            Some(block.time.into()),
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

    // TODO: Enable wallet DB to track "smart" times per `zcashd` logic (Zallet database?).
    let time = blocktime.unwrap_or(timereceived);

    //
    // Below here is equivalent to `ListTransactions` in `zcashd`.
    //

    let details = vec![];

    let hex_tx = {
        let mut bytes = vec![];
        tx.write(&mut bytes).expect("can write to Vec");
        hex::encode(bytes)
    };

    Ok(Transaction {
        txid: txid_str.to_ascii_lowercase(),
        status,
        version: tx.version().header() & 0x7FFFFFFF,
        amount: value_from_zat_balance(amount),
        amount_zat: amount.into(),
        fee,
        confirmations,
        generated,
        blockhash,
        blockindex,
        blocktime,
        expiryheight,
        walletconflicts,
        time,
        timereceived,
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
