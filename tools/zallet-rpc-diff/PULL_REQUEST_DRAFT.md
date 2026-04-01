## Summary

This draft PR introduces the `zallet-rpc-diff` parity harness — a reproducible, production-grade CLI tool that compares `zcashd` and Zallet wallet JSON-RPC responses, classifying each method as `MATCH` / `DIFF` / `MISSING` / `ERROR`.

This addresses the upstream request in [#16](#).

---

## ✅ Milestone 1 Deliverables: Design & Scaffolding

### Design Note
See `tools/zallet-rpc-diff/DESIGN_NOTE.md` which explicitly defines the result categories, `report.json` schema, and the versioned `manifest.toml` format.

### Rust Crate + CLI Skeleton
- `zallet-parity-core` — library with custom `Error` types and result categorization.
- `zallet-parity-cli` — binary `zallet-rpc-diff` with `clap` v4, `color-eyre`, and `tracing`.
- `zallet-parity-testkit` — test helpers suite.

### CI Workflow
`.github/workflows/zallet-rpc-diff.yml` — scoped workspace checks (build, test, fmt, clippy).

---

## ✅ Milestone 2 Deliverables: Execution Engine & Reporting

### Parallel Execution Engine
Implemented an async runner in `zallet-parity-core` using `tokio::task::JoinSet`. It executes the entire manifest suite concurrently against both upstream and target endpoints.

### Deep Semantic Comparison
Integrated `assert-json-diff` to perform recursive comparison of `serde_json::Value` responses. It correctly identifies differences while being resilient to key ordering.

### Premium CLI & Reporting
- **Progress Tracking**: Added `indicatif` progress bars for real-time feedback during large suites.
- **Reporting**: Automatically generates `report.json` (for automation) and a human-readable `report.md` (for PR review).

### Automated Verification Suite
Implemented a robust testing suite using `wiremock` in the `testkit`:
- **Unit Tests**: Verify the parity logic for matches and diffs.
- **Integration Tests**: Verify the E2E CLI workflow against mock RPC nodes.
- **Run with**: `cargo test --workspace`

---

## Questions for Maintainers

1. **Location**: Is `tools/zallet-rpc-diff/` acceptable, or would you prefer `testing/zallet-rpc-diff/`?
2. **Workspace membership**: Should these crates be added to the root workspace `members` list, or kept as a self-contained sub-workspace?
3. **Method suite**: The current `manifest.toml` is a sample; I'm happy to align the v1 allowlist with your priorities.

---

## TODOs (Future Milestones)

- [x] Implement JSON-RPC execution engine (Milestone 2)
- [ ] Normalization + ignore paths (Milestone 3)
- [ ] Expected-differences file (Milestone 4)
- [ ] Method suite v1 + runbook (Milestone 5)
