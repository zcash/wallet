//! PCZT fund method - create a funded PCZT from a transaction proposal.

use std::collections::HashSet;
use std::convert::Infallible;
use std::num::NonZeroU32;

use abscissa_core::Application;
use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::updater::Updater;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zaino_state::FetchServiceSubscriber;
use zcash_address::{ZcashAddress, unified};
use zcash_client_backend::{
    data_api::{
        Account, WalletRead,
        wallet::{
            ConfirmationsPolicy,
            create_pczt_from_proposal,
            input_selection::GreedyInputSelector,
            propose_transfer,
        },
    },
    fees::{DustOutputPolicy, StandardFeeRule, standard::MultiOutputChangeStrategy},
    wallet::OvkPolicy,
    zip321::{Payment, TransactionRequest},
};
use zcash_keys::address::Address;
use transparent::keys::TransparentKeyScope;
use zcash_protocol::{
    PoolType, ShieldedProtocol,
    consensus::{NetworkType, Parameters},
    value::{MAX_MONEY, Zatoshis},
};

use crate::{
    components::{
        database::DbHandle,
        json_rpc::{
            payments::{
                IncompatiblePrivacyPolicy, PrivacyPolicy, enforce_privacy_policy,
                get_account_for_address, parse_memo,
            },
            server::LegacyCode,
            utils::zatoshis_from_value,
        },
        keystore::KeyStore,
    },
    fl,
    prelude::*,
};

pub(crate) type Response = RpcResult<ResultType>;

/// Result of funding a PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct FundResult {
    /// The base64-encoded funded PCZT.
    pub pczt: String,
}

pub(crate) type ResultType = FundResult;

/// Amount parameter for recipients.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct AmountParam {
    /// Recipient address.
    pub address: String,
    /// Amount in ZEC.
    pub amount: serde_json::Value,
    /// Optional memo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

pub(super) const PARAM_FROM_ADDRESS_DESC: &str = "The address to send funds from.";
pub(super) const PARAM_AMOUNTS_DESC: &str = "An array of recipient amounts.";
pub(super) const PARAM_AMOUNTS_REQUIRED: bool = true;
pub(super) const PARAM_MINCONF_DESC: &str = "Minimum confirmations for inputs.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str = "Privacy policy for the transaction.";

/// Creates a funded PCZT from a transaction proposal.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn call(
    mut wallet: DbHandle,
    // Reserved for future use (e.g., hardware wallet signing support)
    _keystore: KeyStore,
    // Reserved for future use (e.g., fetching chain state for expiry height)
    _chain: FetchServiceSubscriber,
    from_address: String,
    amounts: Vec<AmountParam>,
    minconf: Option<u32>,
    privacy_policy: Option<String>,
) -> Response {
    // Validate amounts are not empty
    if amounts.is_empty() {
        return Err(
            LegacyCode::InvalidParameter.with_static("Invalid parameter, amounts array is empty.")
        );
    }

    // Parse amounts into payments
    let mut recipient_addrs = HashSet::new();
    let mut payments = vec![];

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
    }

    if payments.is_empty() {
        return Err(LegacyCode::InvalidParameter.with_static("No recipients"));
    }

    let request = TransactionRequest::new(payments).map_err(|e| {
        LegacyCode::InvalidParameter.with_message(format!("Invalid payment request: {e}"))
    })?;

    // Resolve from_address to account
    let account = {
        let address = Address::decode(wallet.params(), &from_address).ok_or_else(|| {
            LegacyCode::InvalidAddressOrKey.with_static(
                "Invalid from address: should be a taddr, zaddr, or UA.",
            )
        })?;

        get_account_for_address(wallet.as_ref(), &address)
    }?;

    // Parse privacy policy
    let privacy_policy = match privacy_policy.as_deref() {
        Some("LegacyCompat") => Err(LegacyCode::InvalidParameter
            .with_static("LegacyCompat privacy policy is unsupported in Zallet")),
        Some(s) => PrivacyPolicy::from_str(s).ok_or_else(|| {
            LegacyCode::InvalidParameter.with_message(format!("Unknown privacy policy {s}"))
        }),
        None => Ok(PrivacyPolicy::FullPrivacy),
    }?;

    // Get confirmations policy
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

    // Check privacy policy against recipients (same as z_send_many)
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
                    (true, (true, _), _) => (),
                    (false, (true, Some(rest)), _) => max_orchard_available = rest,
                    (true, _, (true, _)) => (),
                    (false, _, (true, Some(rest))) => max_sapling_available = rest,
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

    // Create change strategy
    let change_strategy = MultiOutputChangeStrategy::new(
        StandardFeeRule::Zip317,
        None,
        ShieldedProtocol::Orchard,
        DustOutputPolicy::default(),
        APP.config().note_management.split_policy(),
    );

    // Create input selector and propose transfer
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
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to propose transaction: {e}")))?;

    // Enforce privacy policy on the proposal
    enforce_privacy_policy(&proposal, privacy_policy)?;

    // Check Orchard action limits
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

    // Get derivation info for signing hints
    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::InvalidAddressOrKey
            .with_static("Invalid from address, no payment source found for address.")
    })?;

    // Create PCZT from proposal
    let pczt = create_pczt_from_proposal::<_, _, Infallible, _, Infallible, _>(
        wallet.as_mut(),
        &params,
        account.id(),
        OvkPolicy::Sender,
        &proposal,
    )
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to create PCZT: {}", e)))?;

    // Collect metadata for each transparent input from the proposal
    // (needed for index alignment check after creating PCZT)
    let mut input_metadata = Vec::new();
    for step in proposal.steps() {
        for transparent_input in step.transparent_inputs() {
            let address = transparent_input.recipient_address();
            // Look up the address metadata to get the derivation info
            let meta = wallet.get_transparent_address_metadata(account.id(), address)
                .ok()
                .flatten();
            input_metadata.push(meta);
        }
    }

    // Verify index alignment between proposal and PCZT
    if input_metadata.len() != pczt.transparent().inputs().len() {
        return Err(LegacyCode::Misc.with_static("Internal error: transparent input count mismatch"));
    }

    // Get network ID for proprietary field
    let network_id: u8 = match params.network_type() {
        NetworkType::Main => 0,
        NetworkType::Test => 1,
        NetworkType::Regtest => 2,
    };

    // Use Updater to add versioned proprietary fields
    let updater = Updater::new(pczt);
    let mut pczt = updater
        .update_global_with(|mut global| {
            // Add seed fingerprint (32 bytes)
            global.set_proprietary(
                "zallet.v1.seed_fingerprint".to_string(),
                derivation.seed_fingerprint().to_bytes().to_vec(),
            );
            // Add account index (4 bytes LE)
            global.set_proprietary(
                "zallet.v1.account_index".to_string(),
                u32::from(derivation.account_index()).to_le_bytes().to_vec(),
            );
            // Add network identifier (mainnet=0, testnet=1, regtest=2)
            global.set_proprietary(
                "zallet.v1.network".to_string(),
                vec![network_id],
            );
        })
        .finish();

    // Now update all transparent inputs with their derivation info
    if !input_metadata.is_empty() {
        let updater = Updater::new(pczt);
        pczt = updater
            .update_transparent_with(|mut bundle| {
                for (index, meta) in input_metadata.iter().enumerate() {
                    if let Some(meta) = meta {
                        // Get scope and address_index if this is a derived address
                        if let (Some(scope), Some(address_index)) = (meta.scope(), meta.address_index()) {
                            bundle.update_input_with(index, |mut input| {
                                // Store scope (4 bytes LE)
                                // 0 = external, 1 = internal, 2 = ephemeral
                                let scope_value = if scope == TransparentKeyScope::EXTERNAL {
                                    0u32
                                } else if scope == TransparentKeyScope::INTERNAL {
                                    1u32
                                } else {
                                    2u32 // EPHEMERAL or custom
                                };
                                input.set_proprietary(
                                    "zallet.v1.scope".to_string(),
                                    scope_value.to_le_bytes().to_vec(),
                                );
                                // Store address index (4 bytes LE)
                                input.set_proprietary(
                                    "zallet.v1.address_index".to_string(),
                                    address_index.index().to_le_bytes().to_vec(),
                                );
                                Ok(())
                            })?;
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to update transparent inputs: {e:?}")))?
            .finish();
    }

    // Serialize and encode
    let pczt_bytes = pczt.serialize();
    let pczt_base64 = Base64::encode_string(&pczt_bytes);

    Ok(FundResult { pczt: pczt_base64 })
}
