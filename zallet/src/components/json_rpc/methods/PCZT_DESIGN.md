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
| pczt_fund | ✅ Working | Creates funded PCZT with signing hints |
| pczt_sign | ✅ Working | Signs PCZT using embedded hints |

## Key Design Decision: Signing Hints

The implemented approach:

1. **Use `create_pczt_from_proposal`** - Enabled by adding `serde` feature to `zcash_client_sqlite` (provides `Serialize` for `AccountUuid`)
2. **pczt_fund embeds signing hints** - Seed fingerprint, account index, per-input derivation paths
3. **pczt_sign is a "dumb signer"** - Reads embedded hints and signs, no reverse-mapping needed

This approach:
- Works with hardware wallets (deterministic, displayable metadata)
- Supports air-gapped signing (offline signer doesn't need wallet DB)
- Enables multi-party signing workflows

## Proprietary Field Schema

All proprietary fields use the `zallet.v1.*` namespace for versioning.

### Global Fields (set by pczt_fund)

| Field | Type | Description |
|-------|------|-------------|
| `zallet.v1.seed_fingerprint` | 32 bytes | `SeedFingerprint::to_bytes()` for key derivation |
| `zallet.v1.account_index` | 4 bytes LE | `u32` account index (ZIP-32) |

### Per-Transparent-Input Fields (set by pczt_fund)

| Field | Type | Description |
|-------|------|-------------|
| `zallet.v1.scope` | 4 bytes LE | Key scope: 0=external, 1=internal, 2=ephemeral |
| `zallet.v1.address_index` | 4 bytes LE | Non-hardened child index within scope |

### Reading Fields (in pczt_sign)

```rust
// Global fields
let seed_fp_bytes = pczt.global().proprietary()
    .get("zallet.v1.seed_fingerprint")?;
let seed_fp = SeedFingerprint::from_bytes(seed_fp_bytes.try_into()?);

let account_idx = u32::from_le_bytes(
    pczt.global().proprietary()
        .get("zallet.v1.account_index")?
        .try_into()?
);

// Per-input fields (transparent)
for input in pczt.transparent().inputs() {
    let scope = u32::from_le_bytes(
        input.proprietary().get("zallet.v1.scope")?.try_into()?
    );
    let addr_idx = u32::from_le_bytes(
        input.proprietary().get("zallet.v1.address_index")?.try_into()?
    );
}
```

## PCZT Roles Used

### pczt_fund: Updater role

```rust
use pczt::roles::updater::Updater;

// Add global signing hints
let updater = Updater::new(pczt);
let pczt = updater
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
    .finish();

// Add per-input transparent derivation info
let updater = Updater::new(pczt);
let pczt = updater
    .update_transparent_with(|mut bundle| {
        bundle.update_input_with(index, |mut input| {
            input.set_proprietary("zallet.v1.scope".to_string(), scope_bytes);
            input.set_proprietary("zallet.v1.address_index".to_string(), idx_bytes);
            Ok(())
        })?;
        Ok(())
    })?
    .finish();
```

### pczt_sign: Signer role

```rust
use pczt::roles::signer::Signer;

let mut signer = Signer::new(pczt)?;

// Sign transparent inputs
signer.sign_transparent(index, &secret_key)?;

// Sign Sapling spends
signer.sign_sapling(index, &ask)?;

// Sign Orchard actions
signer.sign_orchard(index, &ask)?;

let signed_pczt = signer.finish();
```

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

## Testing

### Manual Testing Needed

1. **End-to-end workflow**: `pczt_fund` -> `pczt_sign` -> `pczt_finalize` -> `pczt_extract` -> broadcast
2. **Transparent inputs**: Verify signing with external, internal, and ephemeral scope addresses
3. **Shielded signing**: Test Sapling and Orchard spend signing
4. **Multi-input transactions**: Verify all inputs are signed correctly
5. **Error cases**:
   - Wallet locked (should return `WalletUnlockNeeded`)
   - Missing proprietary fields (should return clear error)
   - Wrong seed fingerprint (should fail gracefully)

### Integration Tests to Add

- `test_pczt_fund_creates_valid_pczt`: Verify PCZT structure and proprietary fields
- `test_pczt_sign_signs_all_inputs`: Round-trip fund->sign->verify signatures
- `test_pczt_workflow_transparent`: Full workflow with transparent inputs
- `test_pczt_workflow_shielded`: Full workflow with Sapling/Orchard spends
- `test_pczt_sign_wrong_key`: Verify graceful handling of mismatched keys

### Regtest Testing

- Create funded PCZT, sign, finalize, extract, and broadcast to regtest
- Verify transaction confirms and balances update correctly

## Open Questions

1. **Hardware wallet workflow**: Is proof generation expected on the wallet side, or should there be a separate `pczt_prove` step for external provers?

## References

- pczt crate docs: https://docs.rs/pczt/
- z_send_many.rs: `zallet/src/components/json_rpc/methods/z_send_many.rs`
- keystore.rs: `zallet/src/components/keystore.rs`
