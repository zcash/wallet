# Runbook: `zallet-rpc-diff` Parity Harness

This runbook describes how to run `zallet-rpc-diff` against live zcashd and Zallet
nodes, interpret the output, and extend the method suite over time.

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Installation](#2-installation)
3. [Configuration](#3-configuration)
4. [Running the Harness](#4-running-the-harness)
5. [Interpreting Results](#5-interpreting-results)
6. [Extending the Method Suite](#6-extending-the-method-suite)
7. [Managing Known Differences](#7-managing-known-differences)
8. [Troubleshooting](#8-troubleshooting)

---

## 1. Prerequisites

### Both endpoints must be running

| Endpoint | Typical default port | Notes |
| :--- | :--- | :--- |
| zcashd (upstream) | `8232` (mainnet) | Fully synced to chain tip |
| Zallet (target) | `9067` | Same network as zcashd |

### Same wallet keys

For wallet-level methods (`getbalance`, `z_gettotalbalance`, etc.) to produce
meaningful comparisons, **both nodes must load the same wallet**:

- Export your zcashd wallet keys and import them into Zallet (or vice versa).
- Without key parity, balance/address methods will differ regardless of
  implementation correctness — not a parity gap, just a data gap.

### Network match

Both nodes must point to the same network (mainnet or testnet). Mixing networks
produces meaningless diffs across the board.

### Rust toolchain

```
rustup show        # Confirm you have a recent stable toolchain
cargo --version    # 1.75+
```

---

## 2. Installation

Build from source (within the `zcash/wallet` workspace):

```bash
cd tools/zallet-rpc-diff
cargo build --release
```

The binary will be at `target/release/zallet-rpc-diff`.

Or run directly without installing:

```bash
cargo run --release -- run [flags]
```

---

## 3. Configuration

### Option A: Environment variables (recommended for CI)

```bash
cp examples/endpoints.env .env
# Edit .env with your real endpoint URLs and credentials
source .env
```

`.env` variables:

| Variable | Description | Example |
| :--- | :--- | :--- |
| `UPSTREAM_URL` | zcashd RPC URL (source of truth) | `http://user:pass@127.0.0.1:8232` |
| `TARGET_URL` | Zallet RPC URL (under test) | `http://user:pass@127.0.0.1:9067` |

### Option B: CLI flags

```bash
zallet-rpc-diff run \
  --upstream-url http://user:pass@127.0.0.1:8232 \
  --target-url   http://user:pass@127.0.0.1:9067
```

### Manifest

The default method manifest is `manifest.toml` in the working directory.
Override with `--manifest path/to/manifest.toml`.

See `manifest.toml` for the full method suite v1.
See [`Extending the Method Suite`](#6-extending-the-method-suite) for how to add methods.

### Expected differences

The default expected-differences file is `expected_diffs.toml`.
Override with `--expected-diffs path/to/expected_diffs.toml`.

See `examples/expected_diffs.toml` for documented examples.
If the file is absent, the harness proceeds with no expected differences (all diffs are unexpected).

---

## 4. Running the Harness

### Minimal run (uses defaults)

```bash
zallet-rpc-diff run \
  --upstream-url "$UPSTREAM_URL" \
  --target-url   "$TARGET_URL"
```

Produces:
- `report.json` — machine-readable parity report
- `report.md`   — human-readable Markdown summary

### Custom output path

```bash
zallet-rpc-diff run \
  --upstream-url "$UPSTREAM_URL" \
  --target-url   "$TARGET_URL" \
  --output /tmp/parity-$(date +%Y%m%d).json
```

### With expected differences

```bash
zallet-rpc-diff run \
  --upstream-url   "$UPSTREAM_URL" \
  --target-url     "$TARGET_URL" \
  --expected-diffs examples/expected_diffs.toml
```

### Custom manifest (e.g. a focused subset)

```bash
zallet-rpc-diff run \
  --upstream-url "$UPSTREAM_URL" \
  --target-url   "$TARGET_URL" \
  --manifest     my-wallet-only-manifest.toml
```

### Full example with all flags

```bash
zallet-rpc-diff run \
  --upstream-url   http://user:pass@127.0.0.1:8232 \
  --target-url     http://user:pass@127.0.0.1:9067 \
  --manifest       manifest.toml \
  --expected-diffs expected_diffs.toml \
  --output         report.json
```

### With verbose logging

```bash
RUST_LOG=info zallet-rpc-diff run ...
RUST_LOG=zallet_parity_core=debug zallet-rpc-diff run ...
```

---

## 5. Interpreting Results

### Exit codes

| Code | Meaning |
| :--- | :--- |
| `0` | All methods returned MATCH or EXPECTED_DIFF — no unexpected gaps |
| `1` | At least one DIFF, MISSING, or ERROR result — investigation required |
| `2` | Tool failure (config error, manifest parse error, etc.) |

### Outcome categories

| Label | Meaning | Action |
| :--- | :--- | :--- |
| ✅ **MATCH** | Normalized responses are identical | No action needed |
| ❌ **DIFF** | Responses differ at specific JSON Pointer paths | Investigate; file bug or add to `expected_diffs.toml` if intentional |
| 📋 **EXPECTED_DIFF** | Diff is listed in `expected_diffs.toml` — known/intentional | No action needed; review periodically |
| 🔍 **MISSING** | One endpoint returned `-32601` (method not found) | Zallet has not yet implemented this method, or zcashd has deprecated it |
| ⚠️ **ERROR** | Transport failure or non-method-not-found RPC error | Check node health, RPC auth, and network connectivity |

### Reading `report.json`

```json
{
  "summary": {
    "total": 25,
    "matches": 18,
    "diffs": 2,
    "expected_diffs": 3,
    "missing": 2,
    "errors": 0
  },
  "details": {
    "z_gettotalbalance": {
      "type": "diff",
      "diff_count": 1,
      "diff_paths": ["/private"]
    }
  }
}
```

- `diff_paths` uses JSON Pointer notation (RFC 6901):
  - `/private` → the `private` field at the root of the response
  - `/softforks/0/id` → the `id` field in the first element of `softforks`
- The report does **not** dump full upstream/target payloads by default.
  For verbose debugging, use `RUST_LOG=debug`.

### Reading `report.md`

The Markdown report provides the same data in a human-readable table,
suitable for pasting into GitHub issues or PR descriptions.

### What "results are meaningful" means

Results are only trustworthy when:

1. **Both nodes are on the same network** (mainnet or testnet)
2. **Both nodes are synced to tip** (or within a few blocks)
3. **Both nodes hold the same wallet keys** (for balance/address methods)
4. **Neither node is mid-restart or pruning** during the run

If any of the above is false, expect noise in DIFF and MISSING results
that does not represent a real parity gap.

---

## 6. Extending the Method Suite

To add a new method to the parity suite, add an entry to `manifest.toml`:

```toml
[[methods]]
name = "z_getbalance"
params = ["<your-z-address>"]
tags = ["wallet", "shielded"]
# Optional: strip volatile fields before comparison
ignore_paths = []
```

### Field reference

| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `name` | string | ✅ | JSON-RPC method name |
| `params` | JSON value | ❌ | Parameters to pass (default: `null`) |
| `ignore_paths` | string[] | ❌ | RFC 6901 paths to strip before comparison |
| `tags` | string[] | ❌ | Free-form labels for filtering |

### Guidelines for new entries

- **Start with `ignore_paths` empty**. Run the harness and observe which fields
  differ. Add volatile fields (timestamps, heights, counters) to `ignore_paths`
  only if they are inherently volatile — not if they represent a real gap.
- **Use `expected_diffs.toml`** (not `ignore_paths`) for fields that differ due
  to a known design decision. `ignore_paths` removes fields silently;
  `expected_diffs.toml` makes them visible in the report with a reason.
- **Tag methods** so they can be filtered into subsets for focused runs.

---

## 7. Managing Known Differences

Add entries to `expected_diffs.toml` for any diff that is:
- Intentional (Zallet uses a different schema/format by design)
- Temporary (will be resolved in a future PR, but is tracked)

```toml
[[expected]]
method = "getnetworkinfo"
reason = "Zallet reports its own version string — intentional."
diff_paths = ["/version", "/subversion"]
```

**Rules:**
- If `diff_paths` is empty, **any** diff on that method is expected.
- If `diff_paths` is non-empty, a diff is only `EXPECTED_DIFF` if **all** actual
  diff paths are prefixed by one of the listed paths. Uncovered paths remain `DIFF`.

**Review expected entries periodically.** An `EXPECTED_DIFF` that was added as
"temporary" should be re-evaluated after each Zallet release.

---

## 8. Troubleshooting

### All methods return ERROR

- Check that both endpoint URLs are reachable: `curl -s http://user:pass@127.0.0.1:8232`
- Verify RPC authentication (username/password in URL)
- Check that zcashd/Zallet are running: `ps aux | grep zcashd`

### Unexpected DIFF on volatile fields

- Add the volatile field path to `ignore_paths` in `manifest.toml`.
- Common volatile fields: `/blocks`, `/bestblockhash`, `/verificationprogress`,
  `/timemillis`, `/connections`, `/chainwork`

### Method returns MISSING on Zallet

Zallet is still being developed and does not yet implement all zcashd methods.
This is expected. Track unimplemented methods in an `expected_diffs.toml` entry
or open a Zallet issue.

### Per-request timeout errors (ERROR with "timeout")

The harness applies a 30-second per-request timeout. If a node is slow or
the method is expensive, increase the timeout by modifying the `RpcClient`
initialization (see `client.rs`).

### `manifest parse error`

Verify your TOML is valid. Common mistakes:
- `params` must be a JSON-compatible TOML value (e.g. `params = ["addr"]` not `params = "addr"`)
- `ignore_paths` must be a TOML array: `ignore_paths = ["/field"]`
