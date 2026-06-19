# Zcash dependency-version policy

Keeps this repo's governed Zcash crate pins aligned with their upstream
libraries, and warns in CI when they drift.

## How it works

The source of truth for a library's version is the library's own upstream
repository at its default-branch HEAD. There is no separate central manifest:
each library repo is the hub, so any two consumer repos that each match it also
agree with each other. A consumer can depend on a governed library two ways,
and each is checked accordingly:

- **Registry version pin** (e.g. `orchard = "0.14"` in
  `[workspace.dependencies]`): compared against the library's `[package]
  version` on its `ref` branch.
- **Git pin via `[patch.crates-io]`** (this repo pins the librustzcash crates
  to a fixed `rev`): the pinned commit is compared against the `ref` branch HEAD
  commit, so a pin that has fallen behind `main` is reported as drift.

Two CI jobs:

| Job | File | Trigger | Blocks PR? |
| --- | --- | --- | --- |
| Version consistency | `dep-version-consistency.yml` | PR + daily + manual | Only for `severity = "error"` libraries |
| HEAD compatibility | `dep-head-compat.yml` | Daily + manual | No (early-warning) |

## Shared tool, per-repo config

`dep_policy.py` is intended to be the **same file across every consumer repo**
(to be centralized into a shared action later); only `governed-libs.toml`
differs per repo. Layout differences are expressed in config:

- `consumer_manifest` - manifest to read (here the workspace root `Cargo.toml`).
- `consumer_tables` - dependency tables to scan (here `workspace.dependencies`).
- `head_rewrite_deps` - whether job 2 also rewrites registry pins to git
  (here `false`: job 2 only repoints the `[patch.crates-io]` librustzcash pins,
  matching this repo's intent of tracking librustzcash main while consuming
  released versions of the standalone crates).

## Files

- `governed-libs.toml` - the governed library list and the per-repo settings.
- `dep_policy.py` - stdlib-only checker (`tomllib` + `urllib` + the `git` CLI
  for `ls-remote`). Subcommands:
  - `check` - report drift; emit `::warning::` / `::error::` annotations.
  - `rewrite-to-head` - repoint governed pins to branch HEAD for job 2.
- `allow.toml` (optional) - per-crate exceptions; an allowlisted crate is
  downgraded from `error` to `warn`.

## Run locally

```bash
python3 .github/dep-policy/dep_policy.py check
```

## Severity

Defaults to `warn`. The librustzcash pin is intentionally a fixed `rev`, so it
is usually behind `main`; warn-by-default surfaces that without blocking. Flip a
library to `severity = "error"` once it must track HEAD in lockstep.

## Allowlist example

```toml
# .github/dep-policy/allow.toml
[[allow]]
crate = "zcash_primitives"
reason = "Holding at the current pin until PR #1234 lands"
```

## Known limitations

- Version-pin comparison is against the declared requirement, not the resolved
  `Cargo.lock` graph, so transitive drift is not covered.
- `rewrite-to-head` builds against upstream HEAD; if sibling upstream crates
  have incompatible in-development version requirements, `cargo check` can fail
  for resolution reasons unrelated to this repo's code. That failure is still a
  useful signal but needs a human to read.
