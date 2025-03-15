use std::collections::BTreeMap;

use jsonrpsee::{
    core::RpcResult,
    types::{ErrorCode as RpcErrorCode, ErrorObjectOwned as RpcError},
};
use serde::Serialize;
use zcash_client_backend::data_api::{WalletRead, WalletWrite};
use zcash_protocol::{memo::Memo value::Zatoshis, TxId};

use crate::components::{
    database::DbConnection,
    json_rpc::{server::LegacyCode, value_from_zatoshis},
};

/// Response to a `z_viewtransaction` RPC request.
pub(crate) type Response = RpcResult<Transaction>;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Transaction {
    /// The transaction ID.
    txid: String,

    spends: Vec<Spend>,

    outputs: Vec<Output>,
}

#[derive(Clone, Debug, Serialize)]
struct Spend {
    /// The shielded value pool.
    ///
    /// One of `["sapling", "orchard"]`.
    pool: &'static str,

    /// (sapling) the index of the spend within `vShieldedSpend`.
    #[serde(skip_serializing_if = "Option::is_none")]
    spend: Option<u16>,

    /// (orchard) the index of the action within orchard bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<u16>,

    /// The id for the transaction this note was created in.
    #[serde(rename = "txidPrev")]
    txid_prev: String,

    /// (sapling) the index of the output within the `vShieldedOutput`.
    #[serde(rename = "outputPrev")]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_prev: Option<u16>,

    /// (orchard) the index of the action within the orchard bundle.
    #[serde(rename = "actionPrev")]
    #[serde(skip_serializing_if = "Option::is_none")]
    action_prev: Option<u16>,

    /// The Zcash address involved in the transaction.
    address: String,

    /// The amount in ZEC.
    value: f64,

    /// The amount in zatoshis.
    #[serde(rename = "valueZat")]
    value_zat: u64,
}

#[derive(Clone, Debug, Serialize)]
struct Output {
    /// The shielded value pool.
    ///
    /// One of `["sapling", "orchard"]`.
    pool: &'static str,

    /// (sapling) the index of the output within the vShieldedOutput\n"
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<u16>,

    /// (orchard) the index of the action within the orchard bundle\n"
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<u16>,

    /// The Zcash address involved in the transaction.
    ///
    /// Not included for change outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,

    /// `true` if the output is not for an address in the wallet.
    outgoing: bool,

    /// `true` if this is a change output.
    #[serde(rename = "walletInternal")]
    wallet_internal: bool,

    /// The amount in ZEC.
    value: f64,

    /// The amount in zatoshis.
    #[serde(rename = "valueZat")]
    value_zat: u64,

    /// Hexadecimal string representation of the memo field.
    memo: String,

    /// UTF-8 string representation of memo field (if it contains valid UTF-8).
    #[serde(rename = "memoStr")]
    #[serde(skip_serializing_if = "Option::is_none")]
    memo_str: Option<String>,
}

pub(crate) fn call(wallet: &DbConnection, txid_str: &str) -> Response {
    let txid: TxId = txid_str.parse()?;

    let tx = wallet
        .get_transaction(txid)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or(
            LegacyCode::InvalidAddressOrKey.with_static("Invalid or non-wallet transaction id"),
        )?;

    let mut spends = vec![];
    let mut outputs = vec![];

    // Collect OutgoingViewingKeys for recovering output information.
    let mut sapling_ivks = vec![];
    let mut orchard_ivks = vec![];
    let ovks = vec![];
    for (account_id, ufvk) in wallet
        .get_unified_full_viewing_keys()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        if let Some(t) = ufvk.transparent() {
            let (internal_ovk, external_ovk) = t.ovks_for_shielding();
            ovks.push(internal_ovk.as_bytes());
            ovks.push(external_ovk.as_bytes());
        }
        if let Some(dfvk) = ufvk.sapling() {
            sapling_ivks.push(dfvk.to_ivk(zip32::Scope::External));
            sapling_ivks.push(dfvk.to_ivk(zip32::Scope::Internal));
            ovks.push(dfvk.to_ovk(zip32::Scope::External).0);
            ovks.push(dfvk.to_ovk(zip32::Scope::Internal).0);
        }
        if let Some(fvk) = ufvk.orchard() {
            orchard_ivks.push(fvk.to_ivk(zip32::Scope::External));
            orchard_ivks.push(fvk.to_ivk(zip32::Scope::Internal));
            ovks.push(*fvk.to_ovk(zip32::Scope::External).as_ref());
            ovks.push(*fvk.to_ovk(zip32::Scope::Internal).as_ref());
        }
    }

    // TODO: Sapling
    // if let Some(bundle) = tx.sapling_bundle() {
    //     // Sapling spends
    //     for (spend, i) in bundle.shielded_spends().iter().zip(0..) {
    //         spends.push(Spend {
    //             pool: "sapling",
    //             spend: Some(i),
    //             action: None,
    //             txid_prev: (),
    //             output_prev: (),
    //             action_prev: None,
    //             address: (),
    //             value: (),
    //             value_zat: (),
    //         });
    //     }

    //     // Sapling outputs
    //     for (output, i) in bundle.shielded_outputs().iter().zip(0..) {
    //         outputs.push(Output {
    //             pool: "sapling",
    //             output: Some(i),
    //             action: None,
    //             address: (),
    //             outgoing: (),
    //             wallet_internal: (),
    //             value: (),
    //             value_zat: (),
    //             memo: (),
    //             memo_str: (),
    //         });
    //     }
    // }

    if let Some(bundle) = tx.orchard_bundle() {
        let ovks = ovks
            .iter()
            .map(|k| orchard::keys::OutgoingViewingKey::from(*k))
            .collect::<Vec<_>>();

        let incoming: BTreeMap<usize, (orchard::Note, orchard::Address, [u8; 512])> = bundle
            .decrypt_outputs_with_keys(&orchard_ivks)
            .into_iter()
            .map(|(idx, _, note, addr, memo)| (idx, (note, addr, memo)))
            .collect();

        let outgoing: BTreeMap<usize, (orchard::Note, orchard::Address, [u8; 512])> = bundle
            .recover_outputs_with_ovks(&ovks)
            .into_iter()
            .map(|(idx, _, note, addr, memo)| (idx, (note, addr, memo)))
            .collect();

        for (action, idx) in bundle.actions().iter().zip(0..) {
            let nf = action.nullifier();
            // TODO: Add WalletRead::get_note_with_nullifier
            if let Some(dnote) = wallet.nullifiers.get(nf).and_then(|outpoint| {
                wallet
                    .wallet_received_notes
                    .get(&outpoint.txid)
                    .and_then(|txnotes| txnotes.decrypted_notes.get(&outpoint.action_idx))
            }) {
                // TODO: Add WalletRead::get_address_for_receiver
                // - Returns Ok(None) if the receiver is for an internal address.
                let address = wallet
                    .get_address_for_receiver(dnote.note.recipient())?
                    .map(|addr| addr.encode(wallet.params()));

                spends.push(Spend {
                    pool: "orchard",
                    spend: None,
                    action: Some(idx),
                    txid_prev: *outpoint.txid.as_ref(),
                    output_prev: None,
                    action_prev: Some(outpoint.action_idx),
                    address,
                    value: value_from_zatoshis(dnote.note.value()),
                    value_zat: dnote.note.value().inner(),
                });
            }

            if let Some(((note, addr, memo), is_outgoing)) = incoming
                .get(&idx.into())
                .map(|n| (n, false))
                .or_else(|| outgoing.get(&idx.into()).map(|n| (n, true)))
            {
                // Show the address that was cached at transaction construction as the
                // recipient.
                let address = wallet
                    .get_address_for_receiver(addr)?
                    .map(|addr| addr.encode(wallet.params()));
                let wallet_internal = address.is_none();

                let value = Zatoshis::const_from_u64(note.value().inner());

                let memo_str = match Memo::from_bytes(memo) {
                    Ok(Memo::Text(text_memo)) => Some(text_memo.into()),
                    _ => None,
                };
                let memo = hex::encode(memo);

                outputs.push(Output {
                    pool: "orchard",
                    output: None,
                    action: Some(idx),
                    address,
                    outgoing: is_outgoing,
                    wallet_internal,
                    value: value_from_zatoshis(value),
                    value_zat: value.into_u64(),
                    memo,
                    memo_str,
                });
            }
        }
    }

    Ok(Transaction {
        txid: txid_str.into(),
        spends,
        outputs,
    })
}
