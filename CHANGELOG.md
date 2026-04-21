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
  - `verifymessage`
  - `z_converttex`
  - `z_importaddress`

### Changed
- `getrawtransaction` now correctly reports the fields `asm`, `reqSigs`, `kind`,
  and `addresses` for transparent outputs.
- `z_viewtransaction`: The `outgoing` field is now omitted on outputs that
  `zcashd` didn't include in its response.
- Significant performance improvements to `zallet migrate-zcashd-wallet`.
- `zallet migrate-zcashd-wallet` now accepts `--no-scan` for advanced users who
  cannot reach a chain data source. Keys, accounts, and transaction data are
  still imported; block heights and tree state are not resolved from the chain,
  and address exposure metadata is seeded from each wallet transaction's expiry
  height. A later scan will refine those estimates via the existing `MIN`-merge
  semantics once a chain connection is available.
- `zallet migrate-zcashd-wallet` now soft-degrades if the default (chain-scanning)
  flow cannot connect to a chain data source: it logs a warning with a hint about
  `--no-scan` and continues with keys, accounts, and transaction data only.

### Fixed
- `zallet migrate-zcashd-wallet --no-scan` now marks every transparent address
  observed in a wallet transaction as exposed, so that `listaddresses` returns
  the full HD-derived set after migration even when the chain is not reachable.
- `listaddresses` no longer returns an internal error when the wallet contains
  standalone imported transparent keys (e.g. from a `zcashd` migration).
- No longer crashes in regtest mode when a Sapling or NU5 activation height is
  not defined.

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
