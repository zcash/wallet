# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
During the alpha period, no Semantic Versioning is followed; all releases should
be considered breaking changes.

## [0.1.0-alpha.4] - PLANNED

### Changed
- MSRV is now 1.89.0.

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
