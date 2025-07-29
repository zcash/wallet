use std::collections::BTreeMap;

use documented::Documented;
use jsonrpsee::{
    core::RpcResult,
    types::{ErrorCode as RpcErrorCode, ErrorObjectOwned as RpcError},
};
use schemars::JsonSchema;
use serde::Serialize;
use zcash_client_backend::{
    address::UnifiedAddress,
    data_api::{Account, AccountPurpose, InputSource, NullifierQuery, TargetValue, WalletRead},
    encoding::AddressCodec,
    fees::{orchard::InputView as _, sapling::InputView as _},
    wallet::NoteId,
};
use zcash_protocol::{
    ShieldedProtocol,
    value::{MAX_MONEY, Zatoshis},
};
use zip32::Scope;

use crate::components::{
    database::DbConnection,
    json_rpc::{
        server::LegacyCode,
        utils::{JsonZec, value_from_zatoshis},
    },
};

/// Response to a `z_listunspent` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// A list of unspent notes.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
#[serde(transparent)]
pub(crate) struct ResultType(Vec<UnspentNote>);

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct UnspentNote {
    /// The transaction ID.
    txid: String,

    /// The shielded value pool.
    ///
    /// One of `["sapling", "orchard"]`.
    pool: String,

    /// The Sapling output or Orchard action index.
    outindex: u16,

    /// The number of confirmations.
    confirmations: u32,

    /// `true` if note can be spent by wallet, `false` if address is watchonly.
    spendable: bool,

    /// The unified account ID, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<u32>,

    /// The shielded address.
    ///
    /// Omitted if this note was sent to an internal receiver.
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,

    /// The amount of value in the note.
    amount: JsonZec,

    /// Hexadecimal string representation of memo field.
    memo: String,

    /// UTF-8 string representation of memo field (if it contains valid UTF-8).
    #[serde(rename = "memoStr")]
    #[serde(skip_serializing_if = "Option::is_none")]
    memo_str: Option<String>,

    /// `true` if the address that received the note is also one of the sending addresses.
    ///
    /// Omitted if the note is not spendable.
    #[serde(skip_serializing_if = "Option::is_none")]
    change: Option<bool>,
}

pub(crate) fn call(wallet: &DbConnection) -> Response {
    // Use the height of the maximum scanned block as the anchor height, to emulate a
    // zero-conf transaction in order to select every note in the wallet.
    let anchor_height = match wallet.block_max_scanned().map_err(|e| {
        RpcError::owned(
            LegacyCode::Database.into(),
            "WalletDb::block_max_scanned failed",
            Some(format!("{e}")),
        )
    })? {
        Some(block) => block.block_height(),
        None => return Ok(ResultType(vec![])),
    };

    let mut unspent_notes = vec![];

    for account_id in wallet.get_account_ids().map_err(|e| {
        RpcError::owned(
            LegacyCode::Database.into(),
            "WalletDb::get_account_ids failed",
            Some(format!("{e}")),
        )
    })? {
        let account = wallet
            .get_account(account_id)
            .map_err(|e| {
                RpcError::owned(
                    LegacyCode::Database.into(),
                    "WalletDb::get_account failed",
                    Some(format!("{e}")),
                )
            })?
            // This would be a race condition between this and account deletion.
            .ok_or(RpcErrorCode::InternalError)?;

        let spendable = matches!(account.purpose(), AccountPurpose::Spending { .. });

        // `z_listunspent` assumes a single HD seed.
        // TODO: Fix this limitation.
        let account = account
            .source()
            .key_derivation()
            .map(|derivation| u32::from(derivation.account_index()));

        let notes = wallet
            .select_spendable_notes(
                account_id,
                TargetValue::AtLeast(Zatoshis::const_from_u64(MAX_MONEY)),
                &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                anchor_height,
                &[],
            )
            .map_err(|e| {
                RpcError::owned(
                    LegacyCode::Database.into(),
                    "WalletDb::select_spendable_notes failed",
                    Some(format!("{e}")),
                )
            })?;

        let sapling_nullifiers = wallet
            .get_sapling_nullifiers(NullifierQuery::All)
            .map_err(|e| {
                RpcError::owned(
                    LegacyCode::Database.into(),
                    "WalletDb::get_sapling_nullifiers failed",
                    Some(format!("{e}")),
                )
            })?
            .into_iter()
            .map(|(account_uuid, nf)| (nf, account_uuid))
            .collect::<BTreeMap<_, _>>();

        let get_memo = |txid, protocol, output_index| -> RpcResult<_> {
            Ok(wallet
                .get_memo(NoteId::new(txid, protocol, output_index))
                .map_err(|e| {
                    RpcError::owned(
                        LegacyCode::Database.into(),
                        "WalletDb::get_memo failed",
                        Some(format!("{e}")),
                    )
                })?
                .map(|memo| {
                    (
                        hex::encode(memo.encode().as_array()),
                        match memo {
                            zcash_protocol::memo::Memo::Text(text_memo) => Some(text_memo.into()),
                            _ => None,
                        },
                    )
                })
                .unwrap_or(("TODO: Always enhance every note".into(), None)))
        };

        for note in notes.sapling() {
            let confirmations = wallet
                .get_tx_height(*note.txid())
                .map_err(|e| {
                    RpcError::owned(
                        LegacyCode::Database.into(),
                        "WalletDb::get_tx_height failed",
                        Some(format!("{e}")),
                    )
                })?
                .map(|h| anchor_height + 1 - h)
                .unwrap_or(0);

            let is_internal = note.spending_key_scope() == Scope::Internal;

            let (memo, memo_str) =
                get_memo(*note.txid(), ShieldedProtocol::Sapling, note.output_index())?;

            let change = spendable
                .then(|| {
                    RpcResult::Ok(
                        // Check against the wallet's change address for the associated
                        // unified account.
                        is_internal || {
                            // A Note is marked as "change" if the address that received
                            // it also spent Notes in the same transaction. This will
                            // catch, for instance:
                            // - Change created by spending fractions of Notes (because
                            //   `z_sendmany` sends change to the originating z-address).
                            // - Notes created by consolidation transactions (e.g. using
                            //   `z_mergetoaddress`).
                            // - Notes sent from one address to itself.
                            wallet
                                .get_transaction(*note.txid())
                                // Can error if we haven't enhanced.
                                // TODO: Improve this case so we can raise actual errors.
                                .ok()
                                .flatten()
                                .as_ref()
                                .and_then(|tx| tx.sapling_bundle())
                                .map(|bundle| {
                                    bundle.shielded_spends().iter().any(|spend| {
                                        sapling_nullifiers.get(spend.nullifier())
                                            == Some(&account_id)
                                    })
                                })
                                .unwrap_or(false)
                        },
                    )
                })
                .transpose()?;

            unspent_notes.push(UnspentNote {
                txid: note.txid().to_string(),
                pool: "sapling".into(),
                outindex: note.output_index(),
                confirmations,
                spendable,
                account,
                // TODO: Ensure we generate the same kind of shielded address as `zcashd`.
                address: (!is_internal).then(|| note.note().recipient().encode(wallet.params())),
                amount: value_from_zatoshis(note.value()),
                memo,
                memo_str,
                change,
            })
        }

        for note in notes.orchard() {
            let confirmations = wallet
                .get_tx_height(*note.txid())
                .map_err(|e| {
                    RpcError::owned(
                        LegacyCode::Database.into(),
                        "WalletDb::get_tx_height failed",
                        Some(format!("{e}")),
                    )
                })?
                .map(|h| anchor_height + 1 - h)
                .unwrap_or(0);

            let is_internal = note.spending_key_scope() == Scope::Internal;

            let (memo, memo_str) =
                get_memo(*note.txid(), ShieldedProtocol::Orchard, note.output_index())?;

            unspent_notes.push(UnspentNote {
                txid: note.txid().to_string(),
                pool: "orchard".into(),
                outindex: note.output_index(),
                confirmations,
                spendable,
                account,
                // TODO: Ensure we generate the same kind of shielded address as `zcashd`.
                address: (!is_internal).then(|| {
                    UnifiedAddress::from_receivers(Some(note.note().recipient()), None, None)
                        .expect("valid")
                        .encode(wallet.params())
                }),
                amount: value_from_zatoshis(note.value()),
                memo,
                memo_str,
                change: spendable.then_some(is_internal),
            })
        }
    }

    Ok(ResultType(unspent_notes))
}
