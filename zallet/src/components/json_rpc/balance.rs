use rusqlite::named_params;
use transparent::bundle::TxOut;
use zaino_state::{FetchServiceSubscriber, MempoolKey};
use zcash_client_backend::data_api::WalletRead;
use zcash_client_sqlite::error::SqliteClientError;
use zcash_keys::encoding::AddressCodec;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{consensus::BlockHeight, value::Zatoshis};

use crate::components::database::DbConnection;

/// Coinbase transaction outputs can only be spent after this number of new blocks
/// (consensus rule).
const COINBASE_MATURITY: u32 = 100;

enum IsMine {
    Spendable,
    WatchOnly,
    Either,
}

/// Returns `true` if this output is owned by some account in the wallet and can be spent.
pub(super) fn is_mine_spendable(
    wallet: &DbConnection,
    tx_out: &TxOut,
) -> Result<bool, SqliteClientError> {
    is_mine(wallet, tx_out, IsMine::Spendable)
}

/// Returns `true` if this output is owned by some account in the wallet, but cannot be
/// spent (e.g. because we don't have the spending key, or do not know how to spend it).
#[allow(dead_code)]
pub(super) fn is_mine_watchonly(
    wallet: &DbConnection,
    tx_out: &TxOut,
) -> Result<bool, SqliteClientError> {
    is_mine(wallet, tx_out, IsMine::WatchOnly)
}

/// Returns `true` if this output is owned by some account in the wallet.
pub(super) fn is_mine_spendable_or_watchonly(
    wallet: &DbConnection,
    tx_out: &TxOut,
) -> Result<bool, SqliteClientError> {
    is_mine(wallet, tx_out, IsMine::Either)
}

/// Logically equivalent to [`IsMine(CTxDestination)`] in `zcashd`.
///
/// A transaction is only considered "mine" by virtue of having a P2SH multisig
/// output if we own *all* of the keys involved. Multi-signature transactions that
/// are partially owned (somebody else has a key that can spend them) enable
/// spend-out-from-under-you attacks, especially in shared-wallet situations.
/// Non-P2SH ("bare") multisig outputs never make a transaction "mine".
///
/// [`IsMine(CTxDestination)`]: https://github.com/zcash/zcash/blob/2352fbc1ed650ac4369006bea11f7f20ee046b84/src/script/ismine.cpp#L121
fn is_mine(
    wallet: &DbConnection,
    tx_out: &TxOut,
    include: IsMine,
) -> Result<bool, SqliteClientError> {
    match tx_out.recipient_address() {
        Some(address) => wallet.with_raw(|conn| {
            let mut stmt_addr_mine = conn.prepare(
                "SELECT EXISTS(
                    SELECT 1
                    FROM addresses
                    JOIN accounts ON account_id = accounts.id
                    WHERE cached_transparent_receiver_address = :address
                    AND (
                        :allow_either = 1
                        OR accounts.has_spend_key = :has_spend_key
                    )
                )",
            )?;

            Ok(stmt_addr_mine.query_row(
                named_params! {
                    ":address": address.encode(wallet.params()),
                    ":allow_either": matches!(include, IsMine::Either),
                    ":has_spend_key": matches!(include, IsMine::Spendable),
                },
                |row| row.get(0),
            )?)
        }),
        // TODO: Use `zcash_script` to discover other ways the output might belong to
        // the wallet (like `IsMine(CScript)` does in `zcashd`).
        None => Ok(false),
    }
}

/// Equivalent to [`CTransaction::GetValueOut`] in `zcashd`.
///
/// [`CTransaction::GetValueOut`]: https://github.com/zcash/zcash/blob/2352fbc1ed650ac4369006bea11f7f20ee046b84/src/primitives/transaction.cpp#L214
pub(super) fn wtx_get_value_out(tx: &Transaction) -> Option<Zatoshis> {
    std::iter::empty()
        .chain(
            tx.transparent_bundle()
                .into_iter()
                .flat_map(|bundle| bundle.vout.iter().map(|txout| txout.value)),
        )
        // NB: negative valueBalanceSapling "takes" money from the transparent value pool just as outputs do
        .chain((-tx.sapling_value_balance()).try_into().ok())
        // NB: negative valueBalanceOrchard "takes" money from the transparent value pool just as outputs do
        .chain(
            tx.orchard_bundle()
                .and_then(|b| (-*b.value_balance()).try_into().ok()),
        )
        .chain(tx.sprout_bundle().into_iter().flat_map(|b| {
            b.joinsplits
                .iter()
                // Consensus rule: either `vpub_old` or `vpub_new` MUST be zero.
                // Therefore if `JsDescription::net_value() <= 0`, it is equal to
                // `-vpub_old`.
                .flat_map(|jsdesc| (-jsdesc.net_value()).try_into().ok())
        }))
        .sum()
}

/// Equivalent to [`CWalletTx::GetDebit`] in `zcashd`.
///
/// [`CWalletTx::GetDebit`]: https://github.com/zcash/zcash/blob/2352fbc1ed650ac4369006bea11f7f20ee046b84/src/wallet/wallet.cpp#L4822
pub(super) fn wtx_get_debit(
    wallet: &DbConnection,
    tx: &Transaction,
    is_mine: impl Fn(&DbConnection, &TxOut) -> Result<bool, SqliteClientError>,
) -> Result<Option<Zatoshis>, SqliteClientError> {
    match tx.transparent_bundle() {
        None => Ok(Some(Zatoshis::ZERO)),
        Some(bundle) if bundle.vin.is_empty() => Ok(Some(Zatoshis::ZERO)),
        // Equivalent to `CWallet::GetDebit(CTransaction)` in `zcashd`.
        Some(bundle) => {
            let mut acc = Some(Zatoshis::ZERO);
            for txin in &bundle.vin {
                // Equivalent to `CWallet::GetDebit(CTxIn)` in `zcashd`.
                if let Some(txout) = wallet
                    .get_transaction(*txin.prevout.txid())?
                    .as_ref()
                    .and_then(|prev_tx| prev_tx.transparent_bundle())
                    .and_then(|bundle| bundle.vout.get(txin.prevout.n() as usize))
                {
                    if is_mine(wallet, txout)? {
                        acc = acc + txout.value;
                    }
                }
            }
            Ok(acc)
        }
    }
}

/// Equivalent to [`CWalletTx::GetCredit`] in `zcashd`.
///
/// [`CWalletTx::GetCredit`]: https://github.com/zcash/zcash/blob/2352fbc1ed650ac4369006bea11f7f20ee046b84/src/wallet/wallet.cpp#L4853
pub(super) async fn wtx_get_credit(
    wallet: &DbConnection,
    chain: &FetchServiceSubscriber,
    tx: &Transaction,
    as_of_height: Option<BlockHeight>,
    is_mine: impl Fn(&DbConnection, &TxOut) -> Result<bool, SqliteClientError>,
) -> Result<Option<Zatoshis>, SqliteClientError> {
    match tx.transparent_bundle() {
        None => Ok(Some(Zatoshis::ZERO)),
        // Must wait until coinbase is safely deep enough in the chain before valuing it.
        Some(bundle)
            if bundle.is_coinbase()
                && wtx_get_blocks_to_maturity(wallet, chain, tx, as_of_height).await? > 0 =>
        {
            Ok(Some(Zatoshis::ZERO))
        }
        // Equivalent to `CWallet::GetCredit(CTransaction)` in `zcashd`.
        Some(bundle) => {
            let mut acc = Some(Zatoshis::ZERO);
            for txout in &bundle.vout {
                // Equivalent to `CWallet::GetCredit(CTxOut)` in `zcashd`.
                if is_mine(wallet, txout)? {
                    acc = acc + txout.value;
                }
            }
            Ok(acc)
        }
    }
}

/// Equivalent to [`CWalletTx::IsFromMe`] in `zcashd`.
///
/// [`CWalletTx::IsFromMe`]: https://github.com/zcash/zcash/blob/2352fbc1ed650ac4369006bea11f7f20ee046b84/src/wallet/wallet.cpp#L4967
pub(super) fn wtx_is_from_me(
    wallet: &DbConnection,
    tx: &Transaction,
    is_mine: impl Fn(&DbConnection, &TxOut) -> Result<bool, SqliteClientError>,
) -> Result<bool, SqliteClientError> {
    if wtx_get_debit(wallet, tx, is_mine)?.ok_or_else(|| {
        SqliteClientError::BalanceError(zcash_protocol::value::BalanceError::Overflow)
    })? > Zatoshis::ZERO
    {
        return Ok(true);
    }

    wallet.with_raw(|conn| {
        if let Some(bundle) = tx.sapling_bundle() {
            let mut stmt_note_exists = conn.prepare(
                "SELECT EXISTS(
                    SELECT 1
                    FROM sapling_received_notes
                    WHERE nf = :nf
                )",
            )?;

            for spend in bundle.shielded_spends() {
                if stmt_note_exists
                    .query_row(named_params! {":nf": spend.nullifier().0}, |row| row.get(0))?
                {
                    return Ok(true);
                }
            }
        }

        if let Some(bundle) = tx.orchard_bundle() {
            let mut stmt_note_exists = conn.prepare(
                "SELECT EXISTS(
                    SELECT 1
                    FROM orchard_received_notes
                    WHERE nf = :nf
                )",
            )?;

            for action in bundle.actions() {
                if stmt_note_exists.query_row(
                    named_params! {":nf": action.nullifier().to_bytes()},
                    |row| row.get(0),
                )? {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    })
}

/// Equivalent to [`CMerkleTx::GetBlocksToMaturity`] in `zcashd`.
///
/// [`CMerkleTx::GetBlocksToMaturity`]: https://github.com/zcash/zcash/blob/2352fbc1ed650ac4369006bea11f7f20ee046b84/src/wallet/wallet.cpp#L6915
async fn wtx_get_blocks_to_maturity(
    wallet: &DbConnection,
    chain: &FetchServiceSubscriber,
    tx: &Transaction,
    as_of_height: Option<BlockHeight>,
) -> Result<u32, SqliteClientError> {
    Ok(
        if tx.transparent_bundle().map_or(false, |b| b.is_coinbase()) {
            if let Some(depth) =
                wtx_get_depth_in_main_chain(wallet, chain, tx, as_of_height).await?
            {
                (COINBASE_MATURITY + 1).saturating_sub(depth)
            } else {
                // TODO: Confirm this is what `zcashd` computes for an orphaned coinbase.
                COINBASE_MATURITY + 2
            }
        } else {
            0
        },
    )
}

/// Returns depth of transaction in blockchain.
///
/// - `None`      : not in blockchain, and not in memory pool (conflicted transaction)
/// - `Some(0)`   : in memory pool, waiting to be included in a block (never returned if `as_of_height` is set)
/// - `Some(1..)` : this many blocks deep in the main chain
async fn wtx_get_depth_in_main_chain(
    wallet: &DbConnection,
    chain: &FetchServiceSubscriber,
    tx: &Transaction,
    as_of_height: Option<BlockHeight>,
) -> Result<Option<u32>, SqliteClientError> {
    let chain_height = wallet
        .chain_height()?
        .ok_or_else(|| SqliteClientError::ChainHeightUnknown)?;

    let effective_chain_height = chain_height.min(as_of_height.unwrap_or(chain_height));

    let depth = if let Some(mined_height) = wallet.get_tx_height(tx.txid())? {
        Some(effective_chain_height + 1 - mined_height)
    } else if as_of_height.is_none()
        && chain
            .mempool
            .contains_txid(&MempoolKey(tx.txid().to_string()))
            .await
    {
        Some(0)
    } else {
        None
    };

    Ok(depth)
}
