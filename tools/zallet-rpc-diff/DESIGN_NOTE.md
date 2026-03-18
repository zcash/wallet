# Design Note: `zallet-parity-harness`

## 1. Result Categories
Each RPC method tested will be classified into one of the following categories:

| Category | Description |
| :--- | :--- |
| `MATCH` | Both endpoints returned identical (normalized) JSON results. |
| `DIFF` | Both endpoints returned results, but they differ after normalization. |
| `EXPECTED_DIFF` | A difference was found, but it matches an entry in the `expected_diffs.toml` (e.g., intentional divergence). |
| `MISSING` | The method is missing on one of the endpoints (e.g., "Method not found" RPC error). |
| `ERROR` | A transport error or internal failure occurred during execution. |

## 2. Report Schema (`report.json`)
The output report will follow this JSON structure:

```json
{
  "schema_version": "1",
  "generated_at": "ISO-8601",
  "summary": {
    "total": 0,
    "match": 0,
    "diff": 0,
    "expected_diff": 0,
    "missing": 0,
    "error": 0
  },
  "results": [
    {
      "method": "getbalance",
      "outcome": "MATCH",
      "params": [],
      "details": null
    }
  ]
}
```

## 3. Method-Suite Manifest (`manifest.toml`)
The manifest defines which RPC calls to run.

```toml
version = "1"

[[method]]
name = "getbalance"
params = []
tags = ["core", "balance"]
```
