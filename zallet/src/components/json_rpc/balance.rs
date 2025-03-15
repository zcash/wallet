use transparent::bundle::TxOut;
use zcash_client_backend::data_api::WalletRead;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{consensus::BlockHeight, value::Zatoshis};

use crate::components::database::DbConnection;

/// Equivalent to `CTransaction::GetValueOut` from `zcashd`.
pub(super) fn wtx_get_value_out(tx: &Transaction) -> Option<Zatoshis> {
    tx.transparent_bundle()
        .map(|bundle| bundle.vout.iter().map(|txout| txout.value).sum())
        .unwrap_or(Some(Zatoshis::ZERO))
}

/// Equivalent to `CWalletTx::GetDebit` from `zcashd`.
pub(super) fn wtx_get_debit(
    wallet: &DbConnection,
    tx: &Transaction,
    filter: impl Fn(&TxOut) -> bool,
) -> Option<Zatoshis> {
    match tx.transparent_bundle() {
        None => Some(Zatoshis::ZERO),
        Some(bundle) if bundle.vin.is_empty() => Some(Zatoshis::ZERO),
        Some(bundle) => bundle
            .vin
            .iter()
            .map(|txin| {
                wallet
                    .get_transaction(*txin.prevout.txid())
                    .ok()
                    .flatten()
                    .as_ref()
                    .and_then(|prev_tx| prev_tx.transparent_bundle())
                    .and_then(|bundle| bundle.vout.get(txin.prevout.n() as usize))
                    .filter(|txout| filter(txout))
                    .map(|txout| txout.value)
                    .unwrap_or(Zatoshis::ZERO)
            })
            .sum::<Option<Zatoshis>>(),
    }
}

/// Equivalent to `CWalletTx::GetCredit` from `zcashd`.
pub(super) fn wtx_get_credit(
    wallet: &DbConnection,
    tx: &Transaction,
    as_of_height: Option<BlockHeight>,
    filter: impl Fn(&TxOut) -> bool,
) -> Option<Zatoshis> {
    match tx.transparent_bundle() {
        None => Some(Zatoshis::ZERO),
        // Must wait until coinbase is safely deep enough in the chain before valuing it.
        Some(bundle) if bundle.is_coinbase() && GetBlocksToMaturity(as_of_height) > 0 => {
            Some(Zatoshis::ZERO)
        }
        Some(bundle) => bundle
            .vout
            .iter()
            .map(|txout| {
                if filter(txout) {
                    txout.value
                } else {
                    Zatoshis::ZERO
                }
            })
            .sum::<Option<Zatoshis>>(),
    }
}

pub(super) fn wtx_is_from_me(
    wallet: &DbConnection,
    tx: &Transaction,
    filter: impl Fn(&TxOut) -> bool,
) -> Option<bool> {
    if wtx_get_debit(wallet, tx, filter)? > Zatoshis::ZERO {
        return Some(true);
    }

    if let Some(bundle) = tx.sapling_bundle() {
        for spend in bundle.shielded_spends() {
            // TODO: Check wallet for (spent) nullifiers.
            // spend.nullifier()
        }
    }

    // TODO: Fix bug in `zcashd` where we forgot to add Orchard here.
    if let Some(bundle) = tx.orchard_bundle() {
        for action in bundle.actions() {
            // TODO: Check wallet for (spent) nullifiers.
            // action.nullifier()
        }
    }

    Some(false)
}
