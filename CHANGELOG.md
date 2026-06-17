# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
During the alpha period, no Semantic Versioning is followed; all releases should
be considered breaking changes.

## [0.1.0-alpha.4] - PLANNED

### Added

- RPC methods:
  - `decoderawtransaction`
  - `decodescript`
  - `getwalletstatus`
  - `verifymessage`
  - `z_converttex`
  - `z_importaddress`
  - `z_shieldcoinbase`

### Changed

- **This release is not compatible with wallets created by earlier alpha
  releases.** The embedded Zaino chain indexer made a backwards-incompatible
  change to its database format (zingolabs/zaino#914), which this release pulls
  in. Zallet now refuses to open wallet databases last used by `0.1.0-alpha.3`
  or earlier; start again with a fresh Zallet wallet or a new data directory.
- Updated the Zaino chain indexer to a pre-release `rc-0.4.0` build
  (zingolabs/zaino#1238) that retains NU 6.2 support and adds optional
  ("ephemeral") finalised state. The embedded indexer now runs in ephemeral
  mode, serving finalised chain data directly from the validator instead of
  maintaining a persistent finalised-state database.
- The wallet sync engine has been migrated to Zaino's `ChainIndex` interface,
  and now scans full blocks instead of compact blocks:
  - Shielded outputs are trial-decrypted by a batched decryption engine.
  - Transparent outputs are detected directly while scanning blocks, instead
    of by polling the backing node's address index on every chain tip change.
  - Chain queries made by RPC methods now operate against a stable snapshot of
    the chain state.
- `getrawtransaction` now correctly reports the fields `asm`, `reqSigs`, `kind`,
  and `addresses` for transparent outputs.
- `z_viewtransaction`: The `outgoing` field is now omitted on outputs that
  `zcashd` didn't include in its response.
- Significant performance improvements to `zallet migrate-zcashd-wallet`.
- `zallet migrate-zcashd-wallet` now accepts `--no-scan` to skip chain scanning
  during migration.

### Fixed

- The wallet no longer permanently stops following the chain after a few
  hundred blocks of history. The embedded indexer's finalised-state database
  was configured with a size limit of 0 (a workaround for slow start-up,
  zingolabs/zaino#249), so its sync loop eventually failed with `MDB_MAP_FULL`
  and gave up; running the finalised state ephemerally removes the database
  (and the workaround) entirely.
- `listaddresses` no longer returns an internal error when the wallet contains
  standalone imported transparent keys (e.g. from a `zcashd` migration).
- No longer crashes in regtest mode when a Sapling or NU5 activation height is
  not defined.
- Zallet now refuses to open wallet databases from incompatible earlier alpha
  releases instead of attempting to migrate them.
- `z_sendmany` no longer drop standalone transparent signing keys when the same
  address backs multiple proposal inputs. Keys are now accumulated per address
  rather than overwritten.
- Transparent UTXO ingestion now records `tx_index` for coinbase transactions
  by routing each observed transaction through `decrypt_and_store_transaction`
  in addition to `put_received_transparent_utxo`. This enables
  `z_shieldcoinbase` (and any other consumer of
  `TransparentOutputFilter::CoinbaseOnly`) to correctly identify coinbase
  outputs.
- `z_sendmany` no longer fails with `Query returned no rows` when a proposal
  includes inputs at HD-derived transparent addresses.
  The keystore's standalone-key decryption is now invoked only for addresses
  that were imported standalone; HD-derived addresses are signed for using
  the account's unified spending key.
- `zallet migrate-zcashd-wallet` now migrates transparent addresses that were
  added to the `zcashd` wallet via `importpubkey` or `importaddress <redeemScript>`.
- `zallet migrate-zcashd-wallet` now migrates view-only Sapling keys that were
  added to the `zcashd` wallet via `z_importviewingkey`. Each imported viewing
  key becomes its own view-only account.

## [0.1.0-alpha.3] - 2025-12-15

### Changed

- Finished implementing the following stubbed-out JSON-RPC methods:
  - `z_listaccounts`

### Fixed

- `zallet rpc` can communicate with Zallet again, by using a username and
  password from `zallet.toml` if any are present.

## [0.1.0-alpha.2] - 2025-10-31

### Added

- JSON-RPC authorization mechanisms, matching zcashd:
  - Multi-user (supporting both bare and hashed passwords in `zallet.toml`).

### Fixed

- Several balance calculation bugs have been fixed.
- Bugs related to detection and selection of unspent outputs have been fixed.
- JSON-RPC 1.x responses now use the expected HTTP error codes.
- JSON-RPC error codes now match zcashd more often.

## [0.1.0-alpha.1] - 2025-09-18

Inital alpha release.
