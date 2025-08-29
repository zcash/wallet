use std::collections::BTreeMap;

use documented::Documented;
use jsonrpsee::core::RpcResult;
use orchard::note_encryption::OrchardDomain;
use rusqlite::{OptionalExtension, named_params};
use schemars::JsonSchema;
use serde::Serialize;
use transparent::keys::TransparentKeyScope;
use zaino_state::{FetchServiceSubscriber, ZcashIndexer};
use zcash_address::{
    ToAddress, ZcashAddress,
    unified::{self, Encoding},
};
use zcash_client_backend::data_api::WalletRead;
use zcash_keys::encoding::AddressCodec;
use zcash_note_encryption::{try_note_decryption, try_output_recovery_with_ovk};
use zcash_protocol::{ShieldedProtocol, TxId, consensus::Parameters, memo::Memo, value::Zatoshis};
use zebra_rpc::methods::GetRawTransaction;

use crate::components::{
    database::DbConnection,
    json_rpc::{
        server::LegacyCode,
        utils::{JsonZec, parse_txid, value_from_zatoshis},
    },
};

const POOL_TRANSPARENT: &str = "transparent";
const POOL_SAPLING: &str = "sapling";
const POOL_ORCHARD: &str = "orchard";

/// Response to a `z_viewtransaction` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = Transaction;

/// Detailed information about an in-wallet transaction.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct Transaction {
    /// The transaction ID.
    txid: String,

    /// The inputs to the transaction that the wallet is capable of viewing.
    spends: Vec<Spend>,

    /// The outputs of the transaction that the wallet is capable of viewing.
    outputs: Vec<Output>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct Spend {
    /// The value pool.
    ///
    /// One of `["transparent", "sapling", "orchard"]`.
    pool: &'static str,

    /// (transparent) the index of the spend within `vin`.
    #[serde(rename = "tIn")]
    #[serde(skip_serializing_if = "Option::is_none")]
    t_in: Option<u16>,

    /// (sapling) the index of the spend within `vShieldedSpend`.
    #[serde(skip_serializing_if = "Option::is_none")]
    spend: Option<u16>,

    /// (orchard) the index of the action within orchard bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<u16>,

    /// The id for the transaction this note was created in.
    #[serde(rename = "txidPrev")]
    txid_prev: String,

    /// (transparent) the index of the corresponding output within the previous
    /// transaction's `vout`.
    #[serde(rename = "tOutPrev")]
    #[serde(skip_serializing_if = "Option::is_none")]
    t_out_prev: Option<u16>,

    /// (sapling) the index of the corresponding output within the previous transaction's
    /// `vShieldedOutput`.
    #[serde(rename = "outputPrev")]
    #[serde(skip_serializing_if = "Option::is_none")]
    output_prev: Option<u16>,

    /// (orchard) the index of the corresponding action within the previous transaction's
    /// Orchard bundle.
    #[serde(rename = "actionPrev")]
    #[serde(skip_serializing_if = "Option::is_none")]
    action_prev: Option<u16>,

    /// The Zcash address involved in the transaction.
    ///
    /// Omitted if this note was received on an account-internal address (e.g. change
    /// notes).
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,

    /// The amount in ZEC.
    value: JsonZec,

    /// The amount in zatoshis.
    #[serde(rename = "valueZat")]
    value_zat: u64,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct Output {
    /// The value pool.
    ///
    /// One of `["transparent", "sapling", "orchard"]`.
    pool: &'static str,

    /// (transparent) the index of the output within the `vout`.
    #[serde(rename = "tOut")]
    #[serde(skip_serializing_if = "Option::is_none")]
    t_out: Option<u16>,

    /// (sapling) the index of the output within the `vShieldedOutput`.
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<u16>,

    /// (orchard) the index of the action within the orchard bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<u16>,

    /// The Zcash address involved in the transaction.
    ///
    /// Omitted if this output was received on an account-internal address (e.g. change
    /// outputs), or is a transparent output to a script that is not either P2PKH or P2SH
    /// (and thus doesn't have an address encoding).
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,

    /// `true` if the output is not for an address in the wallet.
    outgoing: bool,

    /// `true` if this is a change output.
    #[serde(rename = "walletInternal")]
    wallet_internal: bool,

    /// The amount in ZEC.
    value: JsonZec,

    /// The amount in zatoshis.
    #[serde(rename = "valueZat")]
    value_zat: u64,

    /// Hexadecimal string representation of the memo field.
    ///
    /// Omitted if this is a transparent output.
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<String>,

    /// UTF-8 string representation of memo field (if it contains valid UTF-8).
    ///
    /// Omitted if this is a transparent output.
    #[serde(rename = "memoStr")]
    #[serde(skip_serializing_if = "Option::is_none")]
    memo_str: Option<String>,
}

pub(super) const PARAM_TXID_DESC: &str = "The ID of the transaction to view.";

pub(crate) async fn call(
    wallet: &DbConnection,
    chain: FetchServiceSubscriber,
    txid_str: &str,
) -> Response {
    let txid = parse_txid(txid_str)?;

    let tx = wallet
        .get_transaction(txid)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or(
            LegacyCode::InvalidAddressOrKey.with_static("Invalid or non-wallet transaction id"),
        )?;

    // TODO: Should we enforce ZIP 212 when viewing outputs of a transaction that is
    // already in the wallet?
    let zip212_enforcement = sapling::note_encryption::Zip212Enforcement::GracePeriod;

    let mut spends = vec![];
    let mut outputs = vec![];

    // Collect viewing keys for recovering output information.
    // - OVKs are used cross-protocol and thus are collected as byte arrays.
    let mut sapling_ivks = vec![];
    let mut orchard_ivks = vec![];
    let mut ovks = vec![];
    for (_, ufvk) in wallet
        .get_unified_full_viewing_keys()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        if let Some(t) = ufvk.transparent() {
            let (internal_ovk, external_ovk) = t.ovks_for_shielding();
            ovks.push((internal_ovk.as_bytes(), zip32::Scope::Internal));
            ovks.push((external_ovk.as_bytes(), zip32::Scope::External));
        }
        for scope in [zip32::Scope::External, zip32::Scope::Internal] {
            if let Some(dfvk) = ufvk.sapling() {
                sapling_ivks.push((
                    sapling::keys::PreparedIncomingViewingKey::new(&dfvk.to_ivk(scope)),
                    scope,
                ));
                ovks.push((dfvk.to_ovk(scope).0, scope));
            }
            if let Some(fvk) = ufvk.orchard() {
                orchard_ivks.push((
                    orchard::keys::PreparedIncomingViewingKey::new(&fvk.to_ivk(scope)),
                    scope,
                ));
                ovks.push((*fvk.to_ovk(scope).as_ref(), scope));
            }
        }
    }

    // TODO: Add `WalletRead::get_note_with_nullifier`
    type OutputInfo = (TxId, u16, Option<String>, Zatoshis);
    fn output_with_nullifier(
        wallet: &DbConnection,
        pool: ShieldedProtocol,
        nf: [u8; 32],
    ) -> RpcResult<Option<OutputInfo>> {
        let (pool_prefix, output_prefix) = match pool {
            ShieldedProtocol::Sapling => ("sapling", "output"),
            ShieldedProtocol::Orchard => ("orchard", "action"),
        };

        wallet
            .with_raw(|conn| {
                conn.query_row(
                    &format!(
                        "SELECT txid, {output_prefix}_index, address, value
                        FROM {pool_prefix}_received_notes
                        JOIN transactions ON tx = id_tx
                        JOIN addresses ON address_id = addresses.id
                        WHERE nf = :nf"
                    ),
                    named_params! {
                        ":nf": nf,
                    },
                    |row| {
                        Ok((
                            TxId::from_bytes(row.get("txid")?),
                            row.get("output_index")?,
                            row.get("address")?,
                            Zatoshis::const_from_u64(row.get("value")?),
                        ))
                    },
                )
            })
            .optional()
            .map_err(|e| {
                LegacyCode::Database.with_message(format!("Failed to fetch spent note: {e:?}"))
            })
    }

    /// Fetches the address that was cached at transaction construction as the recipient.
    // TODO: Move this into `WalletRead`.
    fn sent_to_address(
        wallet: &DbConnection,
        txid: &TxId,
        pool: ShieldedProtocol,
        idx: u16,
        fallback_addr: impl FnOnce() -> Option<String>,
    ) -> RpcResult<Option<String>> {
        Ok(wallet
            .with_raw(|conn| {
                conn.query_row(
                    "SELECT to_address
                            FROM sent_notes
                            JOIN transactions ON tx = id_tx
                            WHERE txid = :txid
                            AND   output_pool = :output_pool
                            AND   output_index = :output_index",
                    named_params! {
                        ":txid": txid.as_ref(),
                        ":output_pool": match pool {
                            ShieldedProtocol::Sapling => 2,
                            ShieldedProtocol::Orchard => 3,
                        },
                        ":output_index": idx,
                    },
                    |row| row.get("to_address"),
                )
            })
            // Allow the `sent_notes` table to not be populated.
            .optional()
            .map_err(|e| {
                LegacyCode::Database.with_message(format!("Failed to fetch sent-to address: {e}"))
            })?
            // If we don't have a cached recipient, fall back on an address that
            // corresponds to the actual receiver.
            .unwrap_or_else(fallback_addr))
    }

    if let Some(bundle) = tx.transparent_bundle() {
        // Transparent inputs
        for (input, idx) in bundle.vin.iter().zip(0u16..) {
            let txid_prev = input.prevout().txid().to_string();

            // TODO: Migrate to a hopefully much nicer Rust API once we migrate to the new Zaino ChainIndex trait.
            let (address, value) = match chain.get_raw_transaction(txid_prev.clone(), Some(1)).await
            {
                Ok(GetRawTransaction::Object(tx)) => {
                    let output = tx
                        .outputs()
                        .get(usize::from(idx))
                        .expect("Zaino should have rejected this earlier");
                    (
                        transparent::address::Script::from(output.script_pub_key().hex())
                            .address()
                            .map(|addr| addr.encode(wallet.params())),
                        Zatoshis::from_nonnegative_i64(output.value_zat())
                            .expect("Zaino should have rejected this earlier"),
                    )
                }
                Ok(_) => unreachable!(),
                Err(_) => todo!(),
            };

            spends.push(Spend {
                pool: POOL_TRANSPARENT,
                t_in: Some(idx),
                spend: None,
                action: None,
                txid_prev,
                t_out_prev: Some(
                    input
                        .prevout()
                        .n()
                        .try_into()
                        .expect("should always be small enough"),
                ),
                output_prev: None,
                action_prev: None,
                address,
                value: value_from_zatoshis(value),
                value_zat: value.into_u64(),
            });
        }

        // Transparent outputs
        for (output, idx) in bundle.vout.iter().zip(0..) {
            let (address, outgoing, wallet_internal) = match output.recipient_address() {
                None => (None, true, false),
                Some(address) => {
                    let wallet_scope =
                        wallet
                            .get_account_ids()
                            .unwrap()
                            .into_iter()
                            .find_map(|account| {
                                match wallet.get_transparent_address_metadata(account, &address) {
                                    Ok(Some(metadata)) => Some(metadata.scope()),
                                    _ => None,
                                }
                            });

                    (
                        Some(address.encode(wallet.params())),
                        wallet_scope.is_none(),
                        matches!(wallet_scope, Some(TransparentKeyScope::INTERNAL)),
                    )
                }
            };

            outputs.push(Output {
                pool: POOL_TRANSPARENT,
                t_out: Some(idx),
                output: None,
                action: None,
                address,
                outgoing,
                wallet_internal,
                value: value_from_zatoshis(output.value()),
                value_zat: output.value().into_u64(),
                memo: None,
                memo_str: None,
            });
        }
    }

    if let Some(bundle) = tx.sapling_bundle() {
        let incoming: BTreeMap<u16, (sapling::Note, Option<sapling::PaymentAddress>, [u8; 512])> =
            bundle
                .shielded_outputs()
                .iter()
                .zip(0..)
                .filter_map(|(output, idx)| {
                    sapling_ivks.iter().find_map(|(ivk, scope)| {
                        sapling::note_encryption::try_sapling_note_decryption(
                            ivk,
                            output,
                            zip212_enforcement,
                        )
                        .map(|(n, a, m)| {
                            (
                                idx,
                                (n, matches!(scope, zip32::Scope::External).then_some(a), m),
                            )
                        })
                    })
                })
                .collect();

        let outgoing: BTreeMap<u16, (sapling::Note, Option<sapling::PaymentAddress>, [u8; 512])> =
            bundle
                .shielded_outputs()
                .iter()
                .zip(0..)
                .filter_map(|(output, idx)| {
                    ovks.iter().find_map(|(ovk, scope)| {
                        sapling::note_encryption::try_sapling_output_recovery(
                            &sapling::keys::OutgoingViewingKey(*ovk),
                            output,
                            zip212_enforcement,
                        )
                        .map(|(n, a, m)| {
                            (
                                idx,
                                (n, matches!(scope, zip32::Scope::External).then_some(a), m),
                            )
                        })
                    })
                })
                .collect();

        // Sapling spends
        for (spend, idx) in bundle.shielded_spends().iter().zip(0..) {
            let spent_note =
                output_with_nullifier(wallet, ShieldedProtocol::Sapling, spend.nullifier().0)?;

            if let Some((txid_prev, output_prev, address, value)) = spent_note {
                spends.push(Spend {
                    pool: POOL_SAPLING,
                    t_in: None,
                    spend: Some(idx),
                    action: None,
                    txid_prev: txid_prev.to_string(),
                    t_out_prev: None,
                    output_prev: Some(output_prev),
                    action_prev: None,
                    address,
                    value: value_from_zatoshis(value),
                    value_zat: value.into_u64(),
                });
            }
        }

        // Sapling outputs
        for (_, idx) in bundle.shielded_outputs().iter().zip(0..) {
            if let Some(((note, addr, memo), is_outgoing)) = incoming
                .get(&idx)
                .map(|n| (n, false))
                .or_else(|| outgoing.get(&idx).map(|n| (n, true)))
            {
                let address =
                    sent_to_address(wallet, &txid, ShieldedProtocol::Sapling, idx, || {
                        addr.map(|address| {
                            ZcashAddress::from_sapling(
                                wallet.params().network_type(),
                                address.to_bytes(),
                            )
                            .encode()
                        })
                    })?;
                let wallet_internal = address.is_none();

                let value = Zatoshis::const_from_u64(note.value().inner());

                let memo_str = match Memo::from_bytes(memo) {
                    Ok(Memo::Text(text_memo)) => Some(text_memo.into()),
                    _ => None,
                };
                let memo = Some(hex::encode(memo));

                outputs.push(Output {
                    pool: POOL_SAPLING,
                    t_out: None,
                    output: Some(idx),
                    action: None,
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

    if let Some(bundle) = tx.orchard_bundle() {
        let incoming: BTreeMap<u16, (orchard::Note, Option<orchard::Address>, [u8; 512])> = bundle
            .actions()
            .iter()
            .zip(0..)
            .filter_map(|(action, idx)| {
                let domain = OrchardDomain::for_action(action);
                orchard_ivks.iter().find_map(|(ivk, scope)| {
                    try_note_decryption(&domain, ivk, action).map(|(n, a, m)| {
                        (
                            idx,
                            (n, matches!(scope, zip32::Scope::External).then_some(a), m),
                        )
                    })
                })
            })
            .collect();

        let outgoing: BTreeMap<u16, (orchard::Note, Option<orchard::Address>, [u8; 512])> = bundle
            .actions()
            .iter()
            .zip(0..)
            .filter_map(|(action, idx)| {
                let domain = OrchardDomain::for_action(action);
                ovks.iter().find_map(move |(ovk, scope)| {
                    try_output_recovery_with_ovk(
                        &domain,
                        &orchard::keys::OutgoingViewingKey::from(*ovk),
                        action,
                        action.cv_net(),
                        &action.encrypted_note().out_ciphertext,
                    )
                    .map(|(n, a, m)| {
                        (
                            idx,
                            (n, matches!(scope, zip32::Scope::External).then_some(a), m),
                        )
                    })
                })
            })
            .collect();

        for (action, idx) in bundle.actions().iter().zip(0..) {
            let spent_note = output_with_nullifier(
                wallet,
                ShieldedProtocol::Orchard,
                action.nullifier().to_bytes(),
            )?;

            if let Some((txid_prev, action_prev, address, value)) = spent_note {
                spends.push(Spend {
                    pool: POOL_ORCHARD,
                    t_in: None,
                    spend: None,
                    action: Some(idx),
                    txid_prev: txid_prev.to_string(),
                    t_out_prev: None,
                    output_prev: None,
                    action_prev: Some(action_prev),
                    address,
                    value: value_from_zatoshis(value),
                    value_zat: value.into_u64(),
                });
            }

            if let Some(((note, addr, memo), is_outgoing)) = incoming
                .get(&idx)
                .map(|n| (n, false))
                .or_else(|| outgoing.get(&idx).map(|n| (n, true)))
            {
                let address =
                    sent_to_address(wallet, &txid, ShieldedProtocol::Orchard, idx, || {
                        addr.map(|address| {
                            ZcashAddress::from_unified(
                                wallet.params().network_type(),
                                unified::Address::try_from_items(vec![unified::Receiver::Orchard(
                                    address.to_raw_address_bytes(),
                                )])
                                .expect("valid"),
                            )
                            .encode()
                        })
                    })?;
                let wallet_internal = address.is_none();

                let value = Zatoshis::const_from_u64(note.value().inner());

                let memo_str = match Memo::from_bytes(memo) {
                    Ok(Memo::Text(text_memo)) => Some(text_memo.into()),
                    _ => None,
                };
                let memo = Some(hex::encode(memo));

                outputs.push(Output {
                    pool: POOL_ORCHARD,
                    t_out: None,
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
