# PR Draft: Add `zallet-rpc-diff` tool for wallet parity testing

## Goal
Establish a reproducible parity harness for comparing `zcashd` and `Zallet` RPC outputs. This addresses the need for measuring migration readiness and identifying compatibility gaps.

## Proposed Changes
- Added `tools/zallet-rpc-diff` workspace.
- `zallet-parity-cli`: Main runner with `color-eyre` and `tracing`.
- `zallet-parity-core`: Engine with custom `Error` types and result categorization.
- `zallet-parity-testkit`: Mocking unit for fixture-based verification.
- `DESIGN_NOTE.md`: Documentation for report schemas and classification logic.

## Verification Plan
- `cargo build --workspace`
- `cargo test --workspace` (Skeleton tests)
- `cargo run -p zallet-parity-cli -- --help`

## Milestone 1 Status
- [x] Design note completed.
- [x] CLI & Core skeleton implemented.
- [x] Custom error handling and telemetry integrated.
- [x] Crate builds successfully.
