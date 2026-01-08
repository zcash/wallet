//! PCZT sign method - sign a PCZT with wallet keys.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::signer::Signer;
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use transparent::keys::{NonHardenedChildIndex, TransparentKeyScope};
use zcash_keys::keys::UnifiedSpendingKey;
use zip32::{fingerprint::SeedFingerprint, AccountId};

use super::pczt_decode::decode_pczt_base64;
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
    /// Indices of transparent inputs that could not be signed.
    pub unsigned_transparent: Vec<usize>,
    /// Indices of Sapling spends that could not be signed.
    pub unsigned_sapling: Vec<usize>,
    /// Indices of Orchard actions that could not be signed.
    pub unsigned_orchard: Vec<usize>,
}

pub(crate) type ResultType = SignResult;

/// Parameters for the pczt_sign RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct SignParams {
    /// The base64-encoded PCZT to sign.
    pub pczt: String,
    /// If true, fail if any inputs cannot be signed.
    pub strict: Option<bool>,
}

pub(super) const PARAM_PCZT_DESC: &str = "The base64-encoded PCZT to sign.";
pub(super) const PARAM_STRICT_DESC: &str = "If true, fail if any inputs cannot be signed.";

/// Signs a PCZT with the wallet's keys.
pub(crate) async fn call(
    wallet: DbHandle,
    keystore: KeyStore,
    pczt_base64: &str,
    strict: Option<bool>,
) -> Response {
    // 1. Parse PCZT from base64
    let pczt = decode_pczt_base64(pczt_base64)?;

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
    let mut unsigned_transparent = Vec::new();
    for (i, derivation_info) in transparent_derivation_info.iter().enumerate() {
        if let Some((scope, addr_idx)) = derivation_info {
            // Try to derive the key - on failure, track as unsigned
            let sk = match usk.transparent().derive_secret_key(*scope, *addr_idx) {
                Ok(sk) => sk,
                Err(_) => {
                    unsigned_transparent.push(i);
                    continue;
                }
            };

            match signer.sign_transparent(i, &sk) {
                Ok(()) => transparent_signed += 1,
                Err(_) => unsigned_transparent.push(i),
            }
        } else {
            // No derivation info available for this input
            unsigned_transparent.push(i);
        }
    }

    // 8. Sign Sapling spends
    let mut sapling_signed = 0;
    let mut unsigned_sapling = Vec::new();
    let sapling_ask = &usk.sapling().expsk.ask;
    for i in 0..sapling_count {
        // Try to sign - if the key doesn't match, it will return an error
        // which we can ignore (the spend may belong to a different key)
        match signer.sign_sapling(i, sapling_ask) {
            Ok(()) => sapling_signed += 1,
            Err(pczt::roles::signer::Error::SaplingSign(
                sapling::pczt::SignerError::WrongSpendAuthorizingKey,
            )) => {
                // This spend doesn't belong to our key, track as unsigned
                unsigned_sapling.push(i);
            }
            Err(_) => {
                // Other error, track as unsigned
                unsigned_sapling.push(i);
            }
        }
    }

    // 9. Sign Orchard actions
    let mut orchard_signed = 0;
    let mut unsigned_orchard = Vec::new();
    let orchard_ask = orchard::keys::SpendAuthorizingKey::from(usk.orchard());
    for i in 0..orchard_count {
        // Try to sign - if the key doesn't match, it will return an error
        // which we can ignore (the action may belong to a different key)
        match signer.sign_orchard(i, &orchard_ask) {
            Ok(()) => orchard_signed += 1,
            Err(pczt::roles::signer::Error::OrchardSign(
                orchard::pczt::SignerError::WrongSpendAuthorizingKey,
            )) => {
                // This action doesn't belong to our key, track as unsigned
                unsigned_orchard.push(i);
            }
            Err(_) => {
                // Other error, track as unsigned
                unsigned_orchard.push(i);
            }
        }
    }

    // 10. Check strict mode
    if strict.unwrap_or(false)
        && (!unsigned_transparent.is_empty()
            || !unsigned_sapling.is_empty()
            || !unsigned_orchard.is_empty())
    {
        return Err(LegacyCode::Verify.with_message(format!(
            "Strict mode: {} transparent, {} sapling, {} orchard inputs remain unsigned",
            unsigned_transparent.len(),
            unsigned_sapling.len(),
            unsigned_orchard.len()
        )));
    }

    // 11. Finish and return signed PCZT
    let signed_pczt = signer.finish();
    let signed_pczt_bytes = signed_pczt.serialize();
    let signed_pczt_base64 = Base64::encode_string(&signed_pczt_bytes);

    Ok(SignResult {
        pczt: signed_pczt_base64,
        transparent_signed,
        sapling_signed,
        orchard_signed,
        unsigned_transparent,
        unsigned_sapling,
        unsigned_orchard,
    })
}
