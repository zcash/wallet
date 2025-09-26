use std::collections::HashSet;
use std::convert::Infallible;
use std::num::NonZeroU32;

use abscissa_core::Application;
use jsonrpsee::core::{JsonValue, RpcResult};
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zaino_state::FetchServiceSubscriber;
use zcash_address::{ZcashAddress, unified};
use zcash_client_backend::data_api::wallet::SpendingKeys;
use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::{
    data_api::{
        Account,
        wallet::{
            ConfirmationsPolicy, create_proposed_transactions,
            input_selection::GreedyInputSelector, propose_transfer,
        },
    },
    fees::{DustOutputPolicy, StandardFeeRule, standard::MultiOutputChangeStrategy},
    wallet::OvkPolicy,
    zip321::{Payment, TransactionRequest},
};
use zcash_client_sqlite::ReceivedNoteId;
use zcash_keys::{address::Address, keys::UnifiedSpendingKey};
use zcash_proofs::prover::LocalTxProver;
use zcash_protocol::{
    PoolType, ShieldedProtocol,
    value::{MAX_MONEY, Zatoshis},
};

use crate::{
    components::{
        database::DbHandle,
        json_rpc::{
            asyncop::{ContextInfo, OperationId},
            payments::{
                IncompatiblePrivacyPolicy, PrivacyPolicy, SendResult, broadcast_transactions,
                enforce_privacy_policy, get_account_for_address, parse_memo,
            },
            server::LegacyCode,
            utils::zatoshis_from_value,
        },
        keystore::KeyStore,
    },
    fl,
    prelude::*,
};

#[cfg(feature = "transparent-key-import")]
use {transparent::address::TransparentAddress, zcash_script::script};

#[derive(Serialize, Deserialize, JsonSchema)]
pub(crate) struct AmountParameter {
    /// A taddr, zaddr, or Unified Address.
    address: String,

    /// The numeric amount in ZEC.
    amount: JsonValue,

    /// If the address is a zaddr, raw data represented in hexadecimal string format. If
    /// the output is being sent to a transparent address, it’s an error to include this
    /// field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memo: Option<String>,
}

/// Response to a `z_sendmany` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = OperationId;

pub(super) const PARAM_FROMADDRESS_DESC: &str =
    "The transparent or shielded address to send the funds from.";
pub(super) const PARAM_AMOUNTS_DESC: &str =
    "An array of JSON objects representing the amounts to send.";
pub(super) const PARAM_AMOUNTS_REQUIRED: bool = true;
pub(super) const PARAM_MINCONF_DESC: &str = "Only use funds confirmed at least this many times.";
pub(super) const PARAM_FEE_DESC: &str = "If set, it must be null.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str =
    "Policy for what information leakage is acceptable.";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call(
    mut wallet: DbHandle,
    keystore: KeyStore,
    chain: FetchServiceSubscriber,
    fromaddress: String,
    amounts: Vec<AmountParameter>,
    minconf: Option<u32>,
    fee: Option<JsonValue>,
    privacy_policy: Option<String>,
) -> RpcResult<(
    Option<ContextInfo>,
    impl Future<Output = RpcResult<SendResult>>,
)> {
    // TODO: Check that Sapling is active, by inspecting height of `chain` snapshot.
    //       https://github.com/zcash/wallet/issues/237

    if amounts.is_empty() {
        return Err(
            LegacyCode::InvalidParameter.with_static("Invalid parameter, amounts array is empty.")
        );
    }

    if fee.is_some() {
        return Err(LegacyCode::InvalidParameter
            .with_static("Zallet always calculates fees internally; the fee field must be null."));
    }

    let mut recipient_addrs = HashSet::new();
    let mut payments = vec![];
    let mut total_out = Zatoshis::ZERO;

    for amount in &amounts {
        let addr: ZcashAddress = amount.address.parse().map_err(|_| {
            LegacyCode::InvalidParameter.with_message(format!(
                "Invalid parameter, unknown address format: {}",
                amount.address,
            ))
        })?;

        if !recipient_addrs.insert(addr.clone()) {
            return Err(LegacyCode::InvalidParameter.with_message(format!(
                "Invalid parameter, duplicated recipient address: {}",
                amount.address,
            )));
        }

        let memo = amount.memo.as_deref().map(parse_memo).transpose()?;
        let value = zatoshis_from_value(&amount.amount)?;

        let payment = Payment::new(addr, value, memo, None, None, vec![]).ok_or_else(|| {
            LegacyCode::InvalidParameter.with_static("Cannot send memo to transparent recipient")
        })?;

        payments.push(payment);
        total_out = (total_out + value)
            .ok_or_else(|| LegacyCode::InvalidParameter.with_static("Value too large"))?;
    }

    if payments.is_empty() {
        return Err(LegacyCode::InvalidParameter.with_static("No recipients"));
    }

    let request = TransactionRequest::new(payments).map_err(|e| {
        // TODO: Map errors to `zcashd` shape.
        LegacyCode::InvalidParameter.with_message(format!("Invalid payment request: {e}"))
    })?;

    let account = match fromaddress.as_str() {
        // Select from the legacy transparent address pool.
        // TODO: Support this if we're going to. https://github.com/zcash/wallet/issues/138
        "ANY_TADDR" => Err(LegacyCode::WalletAccountsUnsupported
            .with_static("The legacy account is currently unsupported for spending from")),
        // Select the account corresponding to the given address.
        _ => {
            let address = Address::decode(wallet.params(), &fromaddress).ok_or_else(|| {
                LegacyCode::InvalidAddressOrKey.with_static(
                "Invalid from address: should be a taddr, zaddr, UA, or the string 'ANY_TADDR'.",
            )
            })?;

            get_account_for_address(wallet.as_ref(), &address)
        }
    }?;

    let privacy_policy = match privacy_policy.as_deref() {
        Some("LegacyCompat") => Err(LegacyCode::InvalidParameter
            .with_static("LegacyCompat privacy policy is unsupported in Zallet")),
        Some(s) => PrivacyPolicy::from_str(s).ok_or_else(|| {
            LegacyCode::InvalidParameter.with_message(format!("Unknown privacy policy {s}"))
        }),
        None => Ok(PrivacyPolicy::FullPrivacy),
    }?;

    // Sanity check for transaction size
    // TODO: https://github.com/zcash/wallet/issues/255

    let confirmations_policy = match minconf {
        Some(minconf) => NonZeroU32::new(minconf).map_or(
            ConfirmationsPolicy::new_symmetrical(NonZeroU32::MIN, true),
            |c| ConfirmationsPolicy::new_symmetrical(c, false),
        ),
        None => {
            APP.config().builder.confirmations_policy().map_err(|_| {
                LegacyCode::Wallet.with_message(
                    "Configuration error: minimum confirmations for spending trusted TXOs cannot exceed that for untrusted TXOs.")
            })?
        }
    };

    let params = *wallet.params();

    // TODO: Fetch the real maximums within the account so we can detect correctly.
    //       https://github.com/zcash/wallet/issues/257
    let mut max_sapling_available = Zatoshis::const_from_u64(MAX_MONEY);
    let mut max_orchard_available = Zatoshis::const_from_u64(MAX_MONEY);

    for payment in request.payments().values() {
        match Address::try_from_zcash_address(&params, payment.recipient_address().clone()) {
            Err(e) => return Err(LegacyCode::InvalidParameter.with_message(e.to_string())),
            Ok(Address::Transparent(_) | Address::Tex(_)) => {
                if !privacy_policy.allow_revealed_recipients() {
                    return Err(IncompatiblePrivacyPolicy::TransparentRecipient.into());
                }
            }
            Ok(Address::Sapling(_)) => {
                match (
                    privacy_policy.allow_revealed_amounts(),
                    max_sapling_available - payment.amount(),
                ) {
                    (false, None) => {
                        return Err(IncompatiblePrivacyPolicy::RevealingSaplingAmount.into());
                    }
                    (false, Some(rest)) => max_sapling_available = rest,
                    (true, _) => (),
                }
            }
            Ok(Address::Unified(ua)) => {
                match (
                    privacy_policy.allow_revealed_amounts(),
                    (
                        ua.receiver_types().contains(&unified::Typecode::Orchard),
                        max_orchard_available - payment.amount(),
                    ),
                    (
                        ua.receiver_types().contains(&unified::Typecode::Sapling),
                        max_sapling_available - payment.amount(),
                    ),
                ) {
                    // The preferred receiver is Orchard, and we either allow revealed
                    // amounts or have sufficient Orchard funds available to avoid it.
                    (true, (true, _), _) => (),
                    (false, (true, Some(rest)), _) => max_orchard_available = rest,

                    // The preferred receiver is Sapling, and we either allow revealed
                    // amounts or have sufficient Sapling funds available to avoid it.
                    (true, _, (true, _)) => (),
                    (false, _, (true, Some(rest))) => max_sapling_available = rest,

                    // We need to reveal something in order to make progress.
                    _ => {
                        if privacy_policy.allow_revealed_recipients() {
                            // Nothing to do here.
                        } else if privacy_policy.allow_revealed_amounts() {
                            return Err(IncompatiblePrivacyPolicy::TransparentReceiver.into());
                        } else {
                            return Err(IncompatiblePrivacyPolicy::RevealingReceiverAmounts.into());
                        }
                    }
                }
            }
        }
    }

    let change_strategy = MultiOutputChangeStrategy::new(
        StandardFeeRule::Zip317,
        None,
        ShieldedProtocol::Orchard,
        DustOutputPolicy::default(),
        APP.config().note_management.split_policy(),
    );

    // TODO: Once `zcash_client_backend` supports spending transparent coins arbitrarily,
    // consider using the privacy policy here to avoid selecting incompatible funds. This
    // would match what `zcashd` did more closely (though we might instead decide to let
    // the selector return its best proposal and then continue to reject afterwards, as we
    // currently do).
    let input_selector = GreedyInputSelector::new();

    let proposal = propose_transfer::<_, _, _, _, Infallible>(
        wallet.as_mut(),
        &params,
        account.id(),
        &input_selector,
        &change_strategy,
        request,
        confirmations_policy,
    )
    // TODO: Map errors to `zcashd` shape.
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to propose transaction: {e}")))?;

    enforce_privacy_policy(&proposal, privacy_policy)?;

    let orchard_actions_limit = APP.config().builder.limits.orchard_actions().into();
    for step in proposal.steps() {
        let orchard_spends = step
            .shielded_inputs()
            .iter()
            .flat_map(|inputs| inputs.notes())
            .filter(|note| note.note().protocol() == ShieldedProtocol::Orchard)
            .count();

        let orchard_outputs = step
            .payment_pools()
            .values()
            .filter(|pool| pool == &&PoolType::ORCHARD)
            .count()
            + step
                .balance()
                .proposed_change()
                .iter()
                .filter(|change| change.output_pool() == PoolType::ORCHARD)
                .count();

        let orchard_actions = orchard_spends.max(orchard_outputs);

        if orchard_actions > orchard_actions_limit {
            let (count, kind) = if orchard_outputs <= orchard_actions_limit {
                (orchard_spends, "inputs")
            } else if orchard_spends <= orchard_actions_limit {
                (orchard_outputs, "outputs")
            } else {
                (orchard_actions, "actions")
            };

            return Err(LegacyCode::Misc.with_message(fl!(
                "err-excess-orchard-actions",
                count = count,
                kind = kind,
                limit = orchard_actions_limit,
                config = "-orchardactionlimit=N",
                bound = format!("N >= %u"),
            )));
        }
    }

    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::InvalidAddressOrKey
            .with_static("Invalid from address, no payment source found for address.")
    })?;

    // Fetch spending key last, to avoid a keystore decryption if unnecessary.
    let seed = keystore
        .decrypt_seed(derivation.seed_fingerprint())
        .await
        .map_err(|e| match e.kind() {
            // TODO: Improve internal error types.
            //       https://github.com/zcash/wallet/issues/256
            crate::error::ErrorKind::Generic if e.to_string() == "Wallet is locked" => {
                LegacyCode::WalletUnlockNeeded.with_message(e.to_string())
            }
            _ => LegacyCode::Database.with_message(e.to_string()),
        })?;
    let usk = UnifiedSpendingKey::from_seed(
        wallet.params(),
        seed.expose_secret(),
        derivation.account_index(),
    )
    .map_err(|e| LegacyCode::InvalidAddressOrKey.with_message(e.to_string()))?;

    #[cfg(feature = "transparent-key-import")]
    let standalone_keys = {
        let mut keys = std::collections::HashMap::new();
        for step in proposal.steps() {
            for input in step.transparent_inputs() {
                if let Some(address) = script::FromChain::parse(&input.txout().script_pubkey().0)
                    .ok()
                    .as_ref()
                    .and_then(TransparentAddress::from_script_from_chain)
                {
                    let secret_key = keystore
                        .decrypt_standalone_transparent_key(&address)
                        .await
                        .map_err(|e| match e.kind() {
                            // TODO: Improve internal error types.
                            crate::error::ErrorKind::Generic
                                if e.to_string() == "Wallet is locked" =>
                            {
                                LegacyCode::WalletUnlockNeeded.with_message(e.to_string())
                            }
                            _ => LegacyCode::Database.with_message(e.to_string()),
                        })?;
                    keys.insert(address, secret_key);
                }
            }
        }
        keys
    };

    // TODO: verify that the proposal satisfies the requested privacy policy

    Ok((
        Some(ContextInfo::new(
            "z_sendmany",
            json!({
                "fromaddress": fromaddress,
                "amounts": amounts,
                "minconf": minconf
            }),
        )),
        run(
            wallet,
            chain,
            proposal,
            SpendingKeys::new(
                usk,
                #[cfg(feature = "zcashd-import")]
                standalone_keys,
            ),
        ),
    ))
}

/// Construct and send the transaction, returning the resulting txid.
/// Errors in transaction construction will throw.
///
/// Notes:
/// 1. #1159 Currently there is no limit set on the number of elements, which could
///    make the tx too large.
/// 2. #1360 Note selection is not optimal.
/// 3. #1277 Spendable notes are not locked, so an operation running in parallel
///    could also try to use them.
async fn run(
    mut wallet: DbHandle,
    chain: FetchServiceSubscriber,
    proposal: Proposal<StandardFeeRule, ReceivedNoteId>,
    spending_keys: SpendingKeys,
) -> RpcResult<SendResult> {
    let prover = LocalTxProver::bundled();
    let (wallet, txids) = crate::spawn_blocking!("z_sendmany prover", move || {
        let params = *wallet.params();
        create_proposed_transactions::<_, _, Infallible, _, Infallible, _>(
            wallet.as_mut(),
            &params,
            &prover,
            &prover,
            &spending_keys,
            OvkPolicy::Sender,
            &proposal,
        )
        .map(|txids| (wallet, txids))
    })
    .await
    // TODO: Map errors to `zcashd` shape.
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to propose transaction: {e}")))?
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to propose transaction: {e}")))?;

    broadcast_transactions(&wallet, chain, txids.into()).await
}
