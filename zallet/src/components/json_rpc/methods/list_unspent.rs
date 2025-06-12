use std::{collections::BTreeMap, num::NonZeroU32};

use documented::Documented;
use jsonrpsee::{
    core::RpcResult,
    types::{ErrorCode as RpcErrorCode, ErrorObjectOwned as RpcError},
};
use schemars::JsonSchema;
use serde::Serialize;
use zcash_client_backend::{
    address::UnifiedAddress,
    data_api::{
        Account, AccountPurpose, InputSource, NullifierQuery, WalletRead, wallet::TargetHeight,
    },
    encoding::AddressCodec,
    fees::{orchard::InputView as _, sapling::InputView as _},
    wallet::NoteId,
};
use zcash_protocol::{ShieldedProtocol, consensus::BlockHeight};
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

    /// The UUID for the wallet account that received this note.
    account_uuid: String,

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

pub(super) const PARAM_MINCONF_DESC: &str =
    "Only include outputs of transactions confirmed at least this many times.";
pub(super) const PARAM_MAXCONF_DESC: &str =
    "Only include outputs of transactions confirmed at most this many times.";
pub(super) const PARAM_INCLUDE_WATCH_ONLY_DESC: &str =
    "Also include outputs received at watch-only addresses.";
pub(super) const PARAM_ADDRESSES_DESC: &str =
    "If non-empty, only outputs received by the provided addresses will be returned.";
pub(super) const PARAM_AS_OF_HEIGHT_DESC: &str = "Execute the query as if it were run when the blockchain was at the height specified by this argument.";

// FIXME: the following parameters are not yet properly supported
// * maxconf
// * include_watch_only
// * addresses
pub(crate) fn call(
    wallet: &DbConnection,
    minconf: Option<u32>,
    _maxconf: Option<u32>,
    _include_watch_only: Option<bool>,
    _addresses: Option<Vec<String>>,
    as_of_height: Option<u32>,
) -> Response {
    let minconf = minconf.unwrap_or(1);
    //let include_watch_only = include_watch_only.unwrap_or(false);
    //let addresses = addresses
    //    .unwrap_or(vec![])
    //    .iter()
    //    .map(|addr| {
    //        Address::decode(wallet.params(), &addr).ok_or_else(|| {
    //            RpcError::owned(
    //                LegacyCode::InvalidParameter.into(),
    //                "Not a valid Zcash address",
    //                Some(addr),
    //            )
    //        })
    //    })
    //    .collect::<Result<Vec<Address>, _>>()?;

    let target_height = match as_of_height.map_or_else(
        || {
            wallet
                .get_target_and_anchor_heights(NonZeroU32::MIN)
                .map_or_else(
                    |e| {
                        Err(RpcError::owned(
                            LegacyCode::Database.into(),
                            "WalletDb::block_max_scanned failed",
                            Some(format!("{e}")),
                        ))
                    },
                    |h_opt| Ok(h_opt.map(|(h, _)| h)),
                )
        },
        |h| Ok(Some(TargetHeight::from(BlockHeight::from(h + 1)))),
    )? {
        Some(h) => h,
        None => {
            return Ok(ResultType(vec![]));
        }
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
            .select_unspent_notes(
                account_id,
                &[ShieldedProtocol::Sapling, ShieldedProtocol::Orchard],
                target_height,
                &[],
            )
            .map_err(|e| {
                RpcError::owned(
                    LegacyCode::Database.into(),
                    "WalletDb::select_unspent_notes failed",
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

        let get_mined_height = |txid| {
            wallet.get_tx_height(txid).map_err(|e| {
                RpcError::owned(
                    LegacyCode::Database.into(),
                    "WalletDb::get_tx_height failed",
                    Some(format!("{e}")),
                )
            })
        };

        for note in notes.sapling() {
            let tx_mined_height = get_mined_height(*note.txid())?;

            // skip notes that do not have sufficient confirmations according to minconf
            if tx_mined_height
                .iter()
                .any(|h| *h > target_height.saturating_sub(minconf))
            {
                continue;
            }

            let confirmations = tx_mined_height
                .map_or(0, |h| u32::from(target_height.saturating_sub(u32::from(h))));
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
                account_uuid: account_id.expose_uuid().to_string(),
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
            let tx_mined_height = get_mined_height(*note.txid())?;

            // skip notes that do not have sufficient confirmations according to minconf
            if tx_mined_height
                .iter()
                .any(|h| *h > target_height.saturating_sub(minconf))
            {
                continue;
            }

            let confirmations = tx_mined_height
                .map_or(0, |h| u32::from(target_height.saturating_sub(u32::from(h))));
            let is_internal = note.spending_key_scope() == Scope::Internal;

            let (memo, memo_str) =
                get_memo(*note.txid(), ShieldedProtocol::Orchard, note.output_index())?;

            unspent_notes.push(UnspentNote {
                txid: note.txid().to_string(),
                pool: "orchard".into(),
                outindex: note.output_index(),
                confirmations,
                spendable,
                account_uuid: account_id.expose_uuid().to_string(),
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
