//! Implementation of the `z_shieldcoinbase` RPC method.

use std::convert::Infallible;
use std::future::Future;

use abscissa_core::Application;
use jsonrpsee::core::{JsonValue, RpcResult};
use secrecy::ExposeSecret;
use transparent::address::TransparentAddress;
use zaino_state::FetchServiceSubscriber;
use zcash_address::unified;
use zcash_client_backend::{
    data_api::{
        Account, TransparentOutputFilter, WalletRead,
        wallet::{
            SpendingKeys, create_proposed_transactions, input_selection::GreedyInputSelector,
            propose_shielding,
        },
    },
    fees::{DustOutputPolicy, StandardFeeRule, standard::MultiOutputChangeStrategy},
    proposal::Proposal,
    wallet::OvkPolicy,
};
use zcash_keys::{address::Address, keys::UnifiedSpendingKey};
use zcash_proofs::prover::LocalTxProver;
use zcash_protocol::{PoolType, ShieldedProtocol, value::Zatoshis};

use crate::{
    components::{
        database::DbHandle,
        json_rpc::{
            asyncop::{ContextInfo, OperationId},
            payments::{
                PrivacyPolicy, SendResult, broadcast_transactions, enforce_privacy_policy,
                get_account_for_address,
            },
            server::LegacyCode,
        },
        keystore::KeyStore,
    },
    fl,
    prelude::*,
};

#[cfg(feature = "transparent-key-import")]
use zcash_script::script;

pub(crate) type ResultType = OperationId;
pub(crate) type Response = RpcResult<ResultType>;

pub(super) const PARAM_FROMADDRESS_DESC: &str = "A wallet-owned transparent address to sweep from, or \"*\" to sweep from all taddrs \
     belonging to the same account as toaddress. Must belong to the same account as toaddress.";
pub(super) const PARAM_TOADDRESS_DESC: &str = "A wallet-owned shielded address used to identify the account. Funds are shielded into \
     the account's internal shielded address, which may differ from this address.";
pub(super) const PARAM_FEE_DESC: &str =
    "If provided, must be null. Zallet always calculates and applies the ZIP-317 fee internally.";
pub(super) const PARAM_LIMIT_DESC: &str = "Accepted for compatibility but currently ignored; does not constrain how many UTXOs are \
     shielded.";
pub(super) const PARAM_MEMO_DESC: &str = "Accepted for compatibility but currently ignored; not stored in the memo field of any new \
     note.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str =
    "Policy for what information leakage is acceptable.";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call(
    mut wallet: DbHandle,
    keystore: KeyStore,
    chain: FetchServiceSubscriber,
    fromaddress: String,
    toaddress: String,
    fee: Option<JsonValue>,
    limit: Option<u32>,
    memo: Option<String>,
    privacy_policy: Option<String>,
) -> RpcResult<(
    Option<ContextInfo>,
    impl Future<Output = RpcResult<SendResult>>,
)> {
    // Validate fee: Zallet always uses ZIP-317 fees internally.
    if fee.is_some() {
        return Err(LegacyCode::InvalidParameter
            .with_static("Zallet always calculates fees internally; the fee field must be null."));
    }

    // Parse the privacy policy.
    // Default to AllowRevealedSenders since shielding always reveals the transparent sender.
    let privacy_policy = match privacy_policy.as_deref() {
        Some("LegacyCompat") => Err(LegacyCode::InvalidParameter
            .with_static("LegacyCompat privacy policy is unsupported in Zallet")),
        Some(s) => PrivacyPolicy::from_str(s).ok_or_else(|| {
            LegacyCode::InvalidParameter.with_message(format!("Unknown privacy policy {s}"))
        }),
        None => Ok(PrivacyPolicy::AllowRevealedSenders),
    }?;

    // TODO(schell): `propose_shielding` does not accept a memo parameter. The memo is
    // accepted here for API compatibility but is currently ignored. Once the backend
    // supports attaching a memo to shielding transactions, wire it through.
    let _memo = memo;

    // TODO(schell): `propose_shielding` does not support a UTXO limit parameter. The
    // limit is accepted here for API compatibility but is currently ignored. Consider
    // pre-filtering UTXOs or extending the backend API to support this.
    let _limit = limit;

    // Validate the destination address: must have at least one shielded receiver.
    let to_address = Address::decode(wallet.params(), &toaddress).ok_or_else(|| {
        LegacyCode::InvalidParameter.with_message(format!(
            "Invalid parameter, unknown address format: {toaddress}"
        ))
    })?;

    match &to_address {
        Address::Transparent(_) | Address::Tex(_) => {
            return Err(LegacyCode::InvalidParameter.with_static(
                "Invalid parameter, toaddress must be a shielded address (Sapling, Orchard, or Unified with shielded receivers).",
            ));
        }
        Address::Unified(ua) => {
            let has_shielded = ua
                .receiver_types()
                .iter()
                .any(|t| matches!(t, unified::Typecode::Sapling | unified::Typecode::Orchard));
            if !has_shielded {
                return Err(LegacyCode::InvalidParameter.with_static(
                    "Invalid parameter, the provided Unified Address has no shielded receivers.",
                ));
            }
        }
        // Sapling addresses are always valid shielded destinations.
        Address::Sapling(_) => {}
    }

    // Look up the account that owns the destination address.
    let account = get_account_for_address(wallet.as_ref(), &to_address)?;

    // Resolve the transparent source addresses.
    // TODO(schell): When fromaddress is "*", we currently only sweep transparent
    // addresses belonging to the same account as toaddress. Check with teammates
    // whether "*" should sweep across all wallet accounts instead (matching zcashd's
    // single-keypool model more closely).
    let from_addrs: Vec<TransparentAddress> = if fromaddress == "*" {
        wallet
            .get_transparent_receivers(account.id(), true, true)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            .into_keys()
            .collect()
    } else {
        // Parse as a transparent address. z_shieldcoinbase only accepts transparent
        // source addresses (or "*").
        let from_address = Address::decode(wallet.params(), &fromaddress).ok_or_else(|| {
            LegacyCode::InvalidAddressOrKey
                .with_static("Invalid from address: should be a taddr or the string \"*\".")
        })?;

        let transparent_addr: TransparentAddress = match from_address {
            Address::Transparent(addr) => addr,
            // Don't allow tex addresses, just as zcashd doesn't allow tex addresses.
            // It would mostly likely be a mistake if the user specifies a tex address here, so we'll err.
            _ => {
                return Err(LegacyCode::InvalidAddressOrKey.with_static(
                    "Invalid from address: z_shieldcoinbase only supports transparent source addresses.",
                ));
            }
        };

        // Verify the transparent address belongs to the same account as toaddress.
        let from_account =
            get_account_for_address(wallet.as_ref(), &Address::Transparent(transparent_addr))?;
        if from_account.id() != account.id() {
            return Err(LegacyCode::InvalidParameter.with_static(
                "Invalid parameter: fromaddress and toaddress must belong to the same account.",
            ));
        }

        vec![transparent_addr]
    };

    if from_addrs.is_empty() {
        return Err(
            LegacyCode::InvalidParameter.with_static("No transparent addresses found to shield.")
        );
    }

    // Set up confirmations policy from the wallet configuration.
    let confirmations_policy = APP.config().builder.confirmations_policy().map_err(|_| {
        LegacyCode::Wallet.with_message(
            "Configuration error: minimum confirmations for spending trusted TXOs \
             cannot exceed that for untrusted TXOs.",
        )
    })?;

    let params = *wallet.params();

    let change_strategy = MultiOutputChangeStrategy::new(
        StandardFeeRule::Zip317,
        None,
        ShieldedProtocol::Orchard,
        DustOutputPolicy::default(),
        APP.config().note_management.split_policy(),
    );

    let input_selector = GreedyInputSelector::new();

    // Create the shielding proposal. Uses Zatoshis::ZERO as the shielding threshold
    // to shield all available coinbase UTXOs. Passes TransparentOutputFilter::CoinbaseOnly
    // to ensure only coinbase UTXOs are selected for shielding.
    let proposal = propose_shielding::<_, _, _, _, Infallible>(
        wallet.as_mut(),
        &params,
        &input_selector,
        &change_strategy,
        Zatoshis::ZERO,
        &from_addrs,
        account.id(),
        confirmations_policy,
        TransparentOutputFilter::CoinbaseOnly,
    )
    // TODO: Map errors to `zcashd` shape.
    .map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Failed to propose shielding transaction: {e}"))
    })?;

    enforce_privacy_policy(&proposal, privacy_policy)?;

    // Check Orchard action limits.
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

    // Derive the spending key for the account.
    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::InvalidAddressOrKey
            .with_static("Invalid address, no payment source found for account.")
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
                            //       https://github.com/zcash/wallet/issues/256
                            crate::error::ErrorKind::Generic
                                if e.to_string() == "Wallet is locked" =>
                            {
                                LegacyCode::WalletUnlockNeeded.with_message(e.to_string())
                            }
                            _ => LegacyCode::Database.with_message(e.to_string()),
                        })?;
                    keys.insert(address, vec![secret_key]);
                }
            }
        }
        keys
    };

    Ok((
        Some(ContextInfo::new(
            "z_shieldcoinbase",
            serde_json::json!({
                "fromaddress": fromaddress,
                "toaddress": toaddress,
                "limit": _limit,
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

/// Construct and broadcast the shielding transaction.
///
/// Notes:
/// 1. Spendable notes/UTXOs are not locked, so an operation running in parallel
///    could also try to use them.
async fn run(
    mut wallet: DbHandle,
    chain: FetchServiceSubscriber,
    proposal: Proposal<StandardFeeRule, Infallible>,
    spending_keys: SpendingKeys,
) -> RpcResult<SendResult> {
    let prover = LocalTxProver::bundled();
    let (wallet, txids) = crate::spawn_blocking!("z_shieldcoinbase runner", move || {
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
    .map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Failed to build shielding transaction: {e}"))
    })?
    .map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Failed to build shielding transaction: {e}"))
    })?;

    broadcast_transactions(&wallet, chain, txids.into()).await
}
