//! PCZT create method — create a PCZT from a transaction proposal.
//!
//! This is the functional replacement for `createrawtransaction` +
//! `fundrawtransaction`: it selects inputs and computes change for a set of
//! recipients, producing a complete (but unproven and unsigned) PCZT.

use std::convert::Infallible;
use std::num::NonZeroU32;

use abscissa_core::Application;
use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::updater::Updater;
use schemars::JsonSchema;
use serde::Serialize;
use transparent::keys::TransparentKeyScope;
use zcash_client_backend::{
    data_api::{
        Account, WalletRead,
        wallet::{ConfirmationsPolicy, create_pczt_from_proposal},
    },
    wallet::OvkPolicy,
};
use zcash_keys::address::Address;

use crate::{
    components::{
        database::DbHandle,
        json_rpc::{
            payments::{
                AmountParameter, PrivacyPolicy, build_request, get_account_for_address,
                propose_and_check,
            },
            server::LegacyCode,
        },
    },
    prelude::*,
};

/// Maximum number of recipients accepted in a single `pczt_create` call.
///
/// A funded transaction is ultimately bounded by the consensus size limit and
/// the configured Orchard action limit, but we reject obviously abusive inputs
/// before doing any proposal work.
const MAX_RECIPIENTS: usize = 1000;

pub(crate) type Response = RpcResult<ResultType>;

/// Result of creating a PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct CreateResult {
    /// The base64-encoded PCZT.
    pub pczt: String,
}

pub(crate) type ResultType = CreateResult;

pub(super) const PARAM_FROM_ADDRESS_DESC: &str = "The address to send funds from.";
pub(super) const PARAM_AMOUNTS_DESC: &str = "An array of recipient amounts.";
pub(super) const PARAM_AMOUNTS_REQUIRED: bool = true;
pub(super) const PARAM_MINCONF_DESC: &str = "Minimum confirmations for inputs.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str = "Privacy policy for the transaction.";

/// Creates a PCZT from a transaction proposal.
pub(crate) async fn call(
    mut wallet: DbHandle,
    from_address: String,
    amounts: Vec<AmountParameter>,
    minconf: Option<u32>,
    privacy_policy: Option<String>,
) -> Response {
    if amounts.len() > MAX_RECIPIENTS {
        return Err(LegacyCode::InvalidParameter.with_message(format!(
            "Too many recipients: {} exceeds maximum of {MAX_RECIPIENTS}",
            amounts.len(),
        )));
    }

    let request = build_request(&amounts)?;

    // Resolve `from_address` to an account.
    let account = {
        let address = Address::decode(wallet.params(), &from_address).ok_or_else(|| {
            LegacyCode::InvalidAddressOrKey
                .with_static("Invalid from address: should be a taddr, zaddr, or UA.")
        })?;

        get_account_for_address(wallet.as_ref(), &address)
    }?;

    let privacy_policy = match privacy_policy.as_deref() {
        Some("LegacyCompat") => Err(LegacyCode::InvalidParameter
            .with_static("LegacyCompat privacy policy is unsupported in Zallet")),
        Some(s) => PrivacyPolicy::from_str(s).ok_or_else(|| {
            LegacyCode::InvalidParameter.with_message(format!("Unknown privacy policy {s}"))
        }),
        None => Ok(PrivacyPolicy::FullPrivacy),
    }?;

    let confirmations_policy = match minconf {
        Some(minconf) => NonZeroU32::new(minconf).map_or(
            ConfirmationsPolicy::new_symmetrical(NonZeroU32::MIN, true),
            |c| ConfirmationsPolicy::new_symmetrical(c, false),
        ),
        None => APP.config().builder.confirmations_policy().map_err(|_| {
            LegacyCode::Wallet.with_message(
                "Configuration error: minimum confirmations for spending trusted TXOs cannot exceed that for untrusted TXOs.")
        })?,
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

    // Derivation info used to populate the zallet signing hints below.
    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::InvalidAddressOrKey
            .with_static("Invalid from address, no payment source found for address.")
    })?;

    // Build the PCZT from the proposal. This selects inputs, computes change,
    // runs IO finalization, and records the native ZIP 32 / BIP 32 derivation
    // metadata, but does not create proofs or signatures.
    let pczt = create_pczt_from_proposal::<_, _, Infallible, _, Infallible, _>(
        wallet.as_mut(),
        &params,
        account.id(),
        OvkPolicy::Sender,
        &proposal,
    )
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to create PCZT: {e}")))?;

    // Collect the per-input transparent derivation info from the proposal, in
    // the same order as the PCZT's transparent inputs.
    let mut input_metadata = Vec::new();
    for step in proposal.steps() {
        for transparent_input in step.transparent_inputs() {
            let address = transparent_input.recipient_address();
            let meta = wallet
                .get_transparent_address_metadata(account.id(), address)
                .ok()
                .flatten();
            input_metadata.push(meta);
        }
    }

    if input_metadata.len() != pczt.transparent().inputs().len() {
        return Err(
            LegacyCode::Misc.with_static("Internal error: transparent input count mismatch")
        );
    }

    // Record signing hints as proprietary fields. The PCZT format does carry
    // native ZIP 32 / BIP 32 derivation metadata (populated above), but pczt
    // 0.7 exposes no public getter for it, so an offline `pczt_sign` cannot read
    // it back. These `zallet.v1.*` fields are a stand-in for that native path
    // until the upstream accessors land.
    let pczt = Updater::new(pczt)
        .update_global_with(|mut global| {
            global.set_proprietary(
                "zallet.v1.seed_fingerprint".to_string(),
                derivation.seed_fingerprint().to_bytes().to_vec(),
            );
            global.set_proprietary(
                "zallet.v1.account_index".to_string(),
                u32::from(derivation.account_index()).to_le_bytes().to_vec(),
            );
        })
        // A no-op when there are no transparent inputs.
        .update_transparent_with(|mut bundle| {
            for (index, meta) in input_metadata.iter().enumerate() {
                if let Some(meta) = meta {
                    // Only derived addresses carry a scope and index.
                    if let (Some(scope), Some(address_index)) = (meta.scope(), meta.address_index())
                    {
                        bundle.update_input_with(index, |mut input| {
                            // scope: 0 = external, 1 = internal, 2 = ephemeral/other.
                            let scope_value = if scope == TransparentKeyScope::EXTERNAL {
                                0u32
                            } else if scope == TransparentKeyScope::INTERNAL {
                                1u32
                            } else {
                                2u32
                            };
                            input.set_proprietary(
                                "zallet.v1.scope".to_string(),
                                scope_value.to_le_bytes().to_vec(),
                            );
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
        .map_err(|_| LegacyCode::Wallet.with_static("Failed to record transparent signing hints"))?
        .finish();

    Ok(CreateResult {
        pczt: Base64::encode_string(&pczt.serialize()),
    })
}
