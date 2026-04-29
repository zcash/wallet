# core2-shim-0.3 (TEMPORARY)

Local shim that re-exports [`corez`](https://github.com/zcash/corez) under the
`core2` name to satisfy transitive dependencies pinned to `core2 ^0.3`. This
crate exists **solely** to keep `cargo build` working without a committed
`Cargo.lock` while the dependency graph still contains crates that haven't
migrated to `corez`.

A sibling shim `vendor/core2-shim-0.4/` provides the same workaround for
`core2 ^0.4` consumers.

## Background

All `core2` versions on crates.io were yanked by their author on 2026-04-14.
Cargo refuses to resolve to yanked versions during a fresh resolve (one
without a `Cargo.lock`), which broke the `Latest build on macOS-latest` CI
job in `zcash/wallet`.

`corez` is the Zcash ecosystem's clean-room replacement for `core2`,
maintained at <https://github.com/zcash/corez>. It is API-compatible with the
subset of `core2::io` that downstream crates use (when the `std` feature is
enabled, `corez::io` re-exports `std::io::*` directly, identical to what
`core2 0.4`'s `std` feature does).

## Who currently relies on this shim

Run `cargo tree --workspace --all-features -i core2` to inspect. As of when
this shim was added, the chains were:

- `equihash 0.2.2` (transitively pulled in via `zcash_primitives 0.24.1`,
  `zcash_proofs 0.24.0`, and `zebra-chain 2.0.0` → `zcash_primitives 0.26`)
  declares `core2 = "^0.3"`. **This shim covers that case.**
- `zaino-state 0.1.2` declares `core2 = "0.4"` directly. The
  `vendor/core2-shim-0.4/` shim covers that case.

## When to remove this shim (and `core2-shim-0.4`)

Both shims and their `[patch.crates-io]` entries can be deleted once **every**
transitive path to `core2` has been eliminated. Concretely:

1. **`equihash 0.2.x` must drop out of the dep graph.** This requires either:
   - Bumping our `zaino-*` git patches to a revision that depends on
     `zcash_primitives >= 0.27` (which uses `equihash 0.3.0` → `corez`).
     Currently blocked: zaino's `dev` branch already requires
     `zebra-chain >= 4.x`, incompatible with our `zebra-chain 2.0.0`
     workspace dep.
   - Bumping our direct `zebra-chain`, `zebra-rpc`, `zebra-state` workspace
     deps to a major release that uses `zcash_primitives >= 0.27`. The latest
     `zebra-chain 6.0.2` on crates.io still uses `equihash 0.2.2`, so this
     also requires upstream zebra to migrate.
2. **`zaino-state`'s direct `core2 = "0.4"` workspace dep must be removed.**
   Already done in zaino dev commit
   [`47356af0`](https://github.com/zingolabs/zaino/commit/47356af0)
   (2026-04-20, "core2 --> corez"). Pulling that requires the same zebra
   bump as above.

To verify removal is safe at any point in the future:

```sh
cargo tree --workspace --all-features -i core2
```

If the command reports "package ID specification `core2` did not match any
packages", both shims can be deleted along with their `[patch.crates-io]`
entries in the workspace-root `Cargo.toml`.

## Do not depend on this crate from any Zallet code

Importing `core2` directly from Zallet sources defeats the purpose: when the
shim is removed, those imports will silently break or pull a different crate.
Use `std::io` (or `corez` directly, behind a `cfg`) instead.
