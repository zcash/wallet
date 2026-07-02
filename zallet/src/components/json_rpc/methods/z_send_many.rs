use std::convert::Infallible;
use std::num::NonZeroU32;

use abscissa_core::Application;
use jsonrpsee::core::{JsonValue, RpcResult};
use secrecy::ExposeSecret;
use serde_json::json;
use zcash_client_backend::data_api::wallet::SpendingKeys;
use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::{
    data_api::{Account, wallet::{ConfirmationsPolicy, create_proposed_transactions}},
    fees::StandardFeeRule,
    wallet::OvkPolicy,
};
use zcash_client_sqlite::ReceivedNoteId;
use zcash_keys::{address::Address, keys::UnifiedSpendingKey};
use zcash_proofs::prover::LocalTxProver;

use crate::{
    components::{
        chain::Chain,
        database::DbHandle,
        json_rpc::{
            asyncop::{ContextInfo, OperationId},
            payments::{
                AmountParameter, PrivacyPolicy, SendResult, broadcast_transactions, build_request,
                get_account_for_address, propose_and_check,
            },
            server::LegacyCode,
        },
        keystore::KeyStore,
    },
    prelude::*,
};

#[cfg(feature = "zcashd-import")]
use crate::components::json_rpc::utils::collect_standalone_transparent_keys;

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
pub(crate) async fn call<C: Chain>(
    mut wallet: DbHandle,
    keystore: KeyStore,
    chain: C,
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

    if fee.is_some() {
        return Err(LegacyCode::InvalidParameter
            .with_static("Zallet always calculates fees internally; the fee field must be null."));
    }

    let request = build_request(&amounts)?;

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
    let proposal = propose_and_check(
        wallet.as_mut(),
        &params,
        account.id(),
        request,
        privacy_policy,
        confirmations_policy,
    )?;

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

    #[cfg(feature = "zcashd-import")]
    let standalone_keys =
        collect_standalone_transparent_keys(wallet.as_ref(), &keystore, account.id(), &proposal)
            .await?;

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
            #[cfg(feature = "zcashd-import")]
            SpendingKeys::new(usk, standalone_keys),
            #[cfg(not(feature = "zcashd-import"))]
            SpendingKeys::from_unified_spending_key(usk),
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
async fn run<C: Chain>(
    mut wallet: DbHandle,
    chain: C,
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
