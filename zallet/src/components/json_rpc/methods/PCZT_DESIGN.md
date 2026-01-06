# PCZT Implementation Design Notes

## Overview

This document captures the design decisions and patterns for implementing PCZT RPC methods in Zallet.

PR: https://github.com/zcash/wallet/pull/354
Issue: https://github.com/zcash/wallet/issues/99

## Current Status

| Method | Status | Notes |
|--------|--------|-------|
| pczt_create | ✅ Working | Creates empty PCZT structure |
| pczt_decode | ✅ Working | Inspects PCZT contents |
| pczt_combine | ✅ Working | Merges multiple PCZTs |
| pczt_finalize | ✅ Working | Runs IO finalization |
| pczt_extract | ✅ Working | Extracts final transaction |
| pczt_fund | ⚠️ Stubbed | Needs signing hints implementation |
| pczt_sign | ⚠️ Stubbed | Needs to read signing hints |

## Key Design Decision: Signing Hints

The recommended approach:

1. **NOT add `Serialize` to `AccountUuid`** - Store metadata in PCZT proprietary fields as bytes/strings instead
2. **pczt_fund embeds signing hints** - Account UUID, seed fingerprint, derivation paths
3. **pczt_sign is a "dumb signer"** - Just reads embedded hints and signs, no reverse-mapping

This approach:
- Avoids pushing serialization requirements upstream
- Works with hardware wallets (deterministic, displayable metadata)
- Supports air-gapped signing (offline signer doesn't need wallet DB)

## PCZT Proprietary Fields API

From pczt 0.5.0:
```rust
use pczt::roles::updater::Updater;

let updater = Updater::new(pczt);
let pczt = updater
    .update_global_with(|g| {
        g.set_proprietary("zallet.account_uuid".to_string(), uuid_bytes);
        g.set_proprietary("zallet.seed_fingerprint".to_string(), fp_bytes);
        g.set_proprietary("zallet.account_index".to_string(), idx_bytes);
    })
    .finish();
```

Per-input proprietary fields also available via:
- `update_transparent_with()`
- `update_sapling_with()` 
- `update_orchard_with()`

## Keystore Pattern (from z_send_many.rs)
```rust
// 1. Get account from address or UUID
let account = get_account_for_address(wallet.as_ref(), &address)?;

// 2. Get derivation info
let derivation = account.source().key_derivation().ok_or_else(|| {
    LegacyCode::InvalidAddressOrKey.with_static("No payment source found")
})?;

// 3. Decrypt seed from keystore  
let seed = keystore
    .decrypt_seed(derivation.seed_fingerprint())
    .await
    .map_err(|e| match e.kind() {
        crate::error::ErrorKind::Generic if e.to_string() == "Wallet is locked" => {
            LegacyCode::WalletUnlockNeeded.with_message(e.to_string())
        }
        _ => LegacyCode::Database.with_message(e.to_string()),
    })?;

// 4. Derive unified spending key
let usk = UnifiedSpendingKey::from_seed(
    wallet.params(),
    seed.expose_secret(),
    derivation.account_index(),
)?;
```

## pczt_fund Implementation Plan
```rust
pub(crate) async fn call(
    wallet: DbHandle,
    keystore: KeyStore,
    chain: FetchServiceSubscriber,
    pczt: Option<String>,          // Existing PCZT to add to (or None)
    from_address: String,
    amounts: Vec<AmountParam>,
    minconf: Option<u32>,
    privacy_policy: Option<String>,
) -> Response {
    // 1. Resolve from_address → account (same as z_send_many)
    let account = get_account_for_address(wallet.as_ref(), &address)?;
    
    // 2. Get derivation info for signing hints
    let derivation = account.source().key_derivation()?;
    
    // 3. Create transaction proposal (reuse z_send_many machinery)
    let proposal = propose_transfer(...)?;
    
    // 4. Create PCZT from proposal
    // BLOCKER: create_pczt_from_proposal requires AccountId: Serialize
    // WORKAROUND: Manually construct using Creator + Updater roles
    
    // 5. Embed signing hints in proprietary fields
    let updater = Updater::new(pczt);
    let pczt = updater
        .update_global_with(|g| {
            g.set_proprietary("zallet.seed_fingerprint".into(), 
                derivation.seed_fingerprint().to_bytes().to_vec());
            g.set_proprietary("zallet.account_index".into(),
                u32::from(derivation.account_index()).to_le_bytes().to_vec());
        })
        .finish();
    
    Ok(FundResult { pczt: Base64::encode_string(&pczt.serialize()) })
}
```

## pczt_sign Implementation Plan
```rust
pub(crate) async fn call(
    wallet: DbHandle,
    keystore: KeyStore,
    pczt_base64: &str,
    account_uuid: Option<String>,  // Override account (optional)
) -> Response {
    // 1. Parse PCZT
    let pczt = Pczt::parse(&Base64::decode_vec(pczt_base64)?)?;
    
    // 2. Read signing hints from proprietary fields
    let seed_fp_bytes = pczt.global().proprietary()
        .get("zallet.seed_fingerprint")
        .ok_or(LegacyCode::InvalidParameter.with_static("Missing signing hints"))?;
    let account_idx_bytes = pczt.global().proprietary()
        .get("zallet.account_index")?;
    
    // 3. Reconstruct derivation info
    let seed_fp = SeedFingerprint::from_bytes(seed_fp_bytes.try_into()?);
    let account_idx = zip32::AccountId::try_from(
        u32::from_le_bytes(account_idx_bytes.try_into()?)
    )?;
    
    // 4. Decrypt seed and derive USK
    let seed = keystore.decrypt_seed(&seed_fp).await?;
    let usk = UnifiedSpendingKey::from_seed(
        wallet.params(),
        seed.expose_secret(),
        account_idx,
    )?;
    
    // 5. Create signer and sign all inputs
    let mut signer = Signer::new(pczt)?;
    
    // Sign transparent inputs
    let transparent_count = pczt.transparent().inputs().len();
    for i in 0..transparent_count {
        // TODO: Get correct derivation index from per-input proprietary fields
        let secret_key = usk.transparent().derive_secret_key(/*index*/)?;
        signer.sign_transparent(i, &secret_key)?;
    }
    
    // Sign Sapling spends
    let sapling_count = pczt.sapling().spends().len();
    for i in 0..sapling_count {
        signer.sign_sapling(i, usk.sapling().expsk().ask())?;
    }
    
    // Sign Orchard actions
    let orchard_count = pczt.orchard().actions().len();
    for i in 0..orchard_count {
        signer.sign_orchard(i, usk.orchard().ask())?;
    }
    
    let signed_pczt = signer.finish();
    
    Ok(SignResult {
        pczt: Base64::encode_string(&signed_pczt.serialize()),
        transparent_signed: transparent_count,
        sapling_signed: sapling_count,
        orchard_signed: orchard_count,
    })
}
```

## Open Questions (for str4d)

1. **AccountUuid serialization**: Should we add `#[derive(Serialize)]` to AccountUuid, or is the proprietary fields approach preferred?

2. **Transparent key derivation**: How do we know which derivation index was used for each transparent input? Should pczt_fund embed per-input derivation paths?

3. **create_pczt_from_proposal**: Should we use this function (requires Serialize bound), or manually construct the PCZT using Creator/Updater roles?

4. **Hardware wallet workflow**: Is proof generation expected on the wallet side, or should there be a separate pczt_prove step for external provers?

## References

- pczt crate 0.5.0: `~/.cargo/registry/src/index.crates.io-*/pczt-0.5.0/`
- z_send_many.rs: `zallet/src/components/json_rpc/methods/z_send_many.rs`
- keystore.rs: `zallet/src/components/keystore.rs`
