//! PCZT sign method - sign a PCZT with wallet keys.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::{roles::signer::Signer, Pczt};
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use transparent::keys::{NonHardenedChildIndex, TransparentKeyScope};
use zcash_keys::keys::UnifiedSpendingKey;
use zip32::{fingerprint::SeedFingerprint, AccountId};

use crate::components::{database::DbHandle, json_rpc::server::LegacyCode, keystore::KeyStore};

pub(crate) type Response = RpcResult<ResultType>;

/// Result of signing a PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct SignResult {
    /// The base64-encoded signed PCZT.
    pub pczt: String,
    /// Number of transparent inputs signed.
    pub transparent_signed: usize,
    /// Number of Sapling spends signed.
    pub sapling_signed: usize,
    /// Number of Orchard actions signed.
    pub orchard_signed: usize,
}

pub(crate) type ResultType = SignResult;

/// Parameters for the pczt_sign RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct SignParams {
    /// The base64-encoded PCZT to sign.
    pub pczt: String,
    /// The account UUID whose keys should sign.
    pub account_uuid: Option<String>,
}

pub(super) const PARAM_PCZT_DESC: &str = "The base64-encoded PCZT to sign.";
pub(super) const PARAM_ACCOUNT_UUID_DESC: &str = "The account UUID whose keys should sign.";

/// Signs a PCZT with the wallet's keys.
pub(crate) async fn call(
    wallet: DbHandle,
    keystore: KeyStore,
    pczt_base64: &str,
    _account_uuid: Option<String>,
) -> Response {
    // 1. Parse PCZT from base64
    let pczt_bytes = Base64::decode_vec(pczt_base64).map_err(|e| {
        LegacyCode::Deserialization.with_message(format!("Invalid base64 encoding: {e}"))
    })?;

    let pczt = Pczt::parse(&pczt_bytes)
        .map_err(|e| LegacyCode::Deserialization.with_message(format!("Invalid PCZT: {e:?}")))?;

    // 2. Read signing hints from proprietary fields in global
    let seed_fp_bytes = pczt
        .global()
        .proprietary()
        .get("zallet.v1.seed_fingerprint")
        .ok_or_else(|| {
            LegacyCode::InvalidParameter
                .with_static("Missing signing hint: zallet.v1.seed_fingerprint")
        })?;

    let seed_fp =
        SeedFingerprint::from_bytes(seed_fp_bytes.as_slice().try_into().map_err(|_| {
            LegacyCode::InvalidParameter.with_static("Invalid seed fingerprint: expected 32 bytes")
        })?);

    let account_idx_bytes = pczt
        .global()
        .proprietary()
        .get("zallet.v1.account_index")
        .ok_or_else(|| {
            LegacyCode::InvalidParameter
                .with_static("Missing signing hint: zallet.v1.account_index")
        })?;

    let account_idx_u32 =
        u32::from_le_bytes(account_idx_bytes.as_slice().try_into().map_err(|_| {
            LegacyCode::InvalidParameter.with_static("Invalid account index: expected 4 bytes")
        })?);

    let account_idx = AccountId::try_from(account_idx_u32).map_err(|_| {
        LegacyCode::InvalidParameter.with_message(format!(
            "Invalid account index: {} is out of range",
            account_idx_u32
        ))
    })?;

    // 3. Read per-input transparent derivation info BEFORE creating Signer
    // (Signer consumes the Pczt, so we need to extract this first)
    let transparent_derivation_info: Vec<Option<(TransparentKeyScope, NonHardenedChildIndex)>> =
        pczt.transparent()
            .inputs()
            .iter()
            .map(|input| {
                let scope_bytes = input.proprietary().get("zallet.v1.scope")?;
                let addr_idx_bytes = input.proprietary().get("zallet.v1.address_index")?;

                let scope_u32 = u32::from_le_bytes(scope_bytes.as_slice().try_into().ok()?);
                let addr_idx_u32 = u32::from_le_bytes(addr_idx_bytes.as_slice().try_into().ok()?);

                let scope = match scope_u32 {
                    0 => TransparentKeyScope::EXTERNAL,
                    1 => TransparentKeyScope::INTERNAL,
                    2 => TransparentKeyScope::EPHEMERAL,
                    _ => return None,
                };

                let addr_idx = NonHardenedChildIndex::from_index(addr_idx_u32)?;

                Some((scope, addr_idx))
            })
            .collect();

    // Count inputs before Signer consumes the PCZT
    let sapling_count = pczt.sapling().spends().len();
    let orchard_count = pczt.orchard().actions().len();

    // 4. Decrypt seed from keystore
    let seed = keystore
        .decrypt_seed(&seed_fp)
        .await
        .map_err(|e| match e.kind() {
            crate::error::ErrorKind::Generic if e.to_string() == "Wallet is locked" => {
                LegacyCode::WalletUnlockNeeded.with_message(e.to_string())
            }
            _ => LegacyCode::Database.with_message(e.to_string()),
        })?;

    // 5. Derive UnifiedSpendingKey
    let usk = UnifiedSpendingKey::from_seed(wallet.params(), seed.expose_secret(), account_idx)
        .map_err(|e| {
            LegacyCode::InvalidAddressOrKey
                .with_message(format!("Failed to derive spending key: {e}"))
        })?;

    // 6. Create Signer (consumes the PCZT)
    let mut signer = Signer::new(pczt)
        .map_err(|e| LegacyCode::Verify.with_message(format!("Failed to create signer: {e:?}")))?;

    // 7. Sign transparent inputs
    let mut transparent_signed = 0;
    for (i, derivation_info) in transparent_derivation_info.iter().enumerate() {
        if let Some((scope, addr_idx)) = derivation_info {
            let sk = usk
                .transparent()
                .derive_secret_key(*scope, *addr_idx)
                .map_err(|e| {
                    LegacyCode::InvalidAddressOrKey.with_message(format!(
                        "Failed to derive transparent key for input {}: {}",
                        i, e
                    ))
                })?;

            signer.sign_transparent(i, &sk).map_err(|e| {
                LegacyCode::Verify
                    .with_message(format!("Failed to sign transparent input {}: {:?}", i, e))
            })?;

            transparent_signed += 1;
        }
    }

    // 8. Sign Sapling spends
    let mut sapling_signed = 0;
    let sapling_ask = &usk.sapling().expsk.ask;
    for i in 0..sapling_count {
        // Try to sign - if the key doesn't match, it will return an error
        // which we can ignore (the spend may belong to a different key)
        match signer.sign_sapling(i, sapling_ask) {
            Ok(()) => sapling_signed += 1,
            Err(pczt::roles::signer::Error::SaplingSign(
                sapling::pczt::SignerError::WrongSpendAuthorizingKey,
            )) => {
                // This spend doesn't belong to our key, skip it
            }
            Err(e) => {
                return Err(LegacyCode::Verify
                    .with_message(format!("Failed to sign Sapling spend {}: {:?}", i, e)));
            }
        }
    }

    // 9. Sign Orchard actions
    let mut orchard_signed = 0;
    let orchard_ask = orchard::keys::SpendAuthorizingKey::from(usk.orchard());
    for i in 0..orchard_count {
        // Try to sign - if the key doesn't match, it will return an error
        // which we can ignore (the action may belong to a different key)
        match signer.sign_orchard(i, &orchard_ask) {
            Ok(()) => orchard_signed += 1,
            Err(pczt::roles::signer::Error::OrchardSign(
                orchard::pczt::SignerError::WrongSpendAuthorizingKey,
            )) => {
                // This action doesn't belong to our key, skip it
            }
            Err(e) => {
                return Err(LegacyCode::Verify
                    .with_message(format!("Failed to sign Orchard action {}: {:?}", i, e)));
            }
        }
    }

    // 10. Finish and return signed PCZT
    let signed_pczt = signer.finish();
    let signed_pczt_bytes = signed_pczt.serialize();
    let signed_pczt_base64 = Base64::encode_string(&signed_pczt_bytes);

    Ok(SignResult {
        pczt: signed_pczt_base64,
        transparent_signed,
        sapling_signed,
        orchard_signed,
    })
}
