# Installation

There are multiple ways to install the `zallet` binary. The table below has a summary of
the simplest options:

| Environment | CLI command |
|-------------|-------------|
| Debian | [Debian packages](debian.md) |
| Ubuntu | [Debian packages](debian.md) |

> Help from new packagers is very welcome. However, please note that Zallet is currently
> ALPHA software, and is rapidly changing. If you create a Zallet package before the 1.0.0
> production release, please ensure you mark it as alpha software and regularly update it.

## Chain-data backends (`zallet` vs `zallet-zaino`)

Zallet reads chain data through one of two **mutually exclusive** backends, selected at
compile time. The official Docker image and Debian package ship **both** binaries side by
side, so you pick a backend by which binary you run:

| Binary | Backend (Cargo feature) | How it reaches the chain | Regtest |
|--------|-------------------------|--------------------------|---------|
| `zallet` | `zebra-state` (default) | Reads finalized state directly from a co-located `zebrad`'s `ReadStateService`. **Requires a `zebrad` built with the `indexer` feature** and a shared state directory. | No |
| `zallet-zebra-state` | `zebra-state` | Symlink alias of `zallet` — same binary, explicit name for the default backend. | No |
| `zallet-zaino` | `zaino` | Talks to Zebra over JSON-RPC; Zebra and Zallet can run as separate services/containers. | Yes |

**Which one?** `zallet` (the default) is the `zebra-state` backend and **only works
against a `zebrad` that was built with the non-default `indexer` feature** (plus a shared
state directory). If your `zebrad` is **not** built with `indexer` — including the stock
`zfnd/zebra` images and the [z3](https://github.com/ZcashFoundation/z3) stack, or any
setup where Zebra and Zallet run as separate containers talking over JSON-RPC, or when you
need regtest — use **`zallet-zaino`** instead. `zallet-zebra-state` is just an explicit
alias of `zallet` for when you want the backend named. All three share the same CLI
surface, config format, and subcommands; only the chain-data backend differs.

The pre-compiled standalone binaries on the GitHub Releases page follow the same split:
`zallet-<version>-linux-<arch>` (zebra-state) and `zallet-<version>-linux-<arch>-zaino`
(zaino).

## Pre-compiled binaries

> WARNING: This approach does not have automatic updates.

Executable binaries are available for download on the [GitHub Releases page].

[GitHub Releases page]: https://github.com/zcash/wallet/releases

## Build from source using Rust

> WARNING: This approach does not have automatic updates.

To build Zallet from source, you will first need to install Rust and Cargo. Follow the
instructions on the [Rust installation page]. Zallet currently requires at least Rust
version 1.85.

> WARNING: The following does not yet work because Zallet cannot be published to
> [crates.io] while it has unpublished dependencies. This will be fixed during the alpha
> phase. In the meantime, follow the instructions to install the latest development
> version.

Once you have installed Rust, the following command can be used to build and install
Zallet:

```
cargo install --locked zallet
```

This will automatically download Zallet from [crates.io], build it, and install it in
Cargo's global binary directory (`~/.cargo/bin/` by default).

To update, run `cargo install zallet` again. It will check if there is a newer version,
and re-install Zallet if a new version is found. You will need to shut down and restart
any [running Zallet instances](../../cli/start.md) to apply the new version.

To uninstall, run the command `cargo uninstall zallet`. This will only uninstall the
binary, and will not alter any existing wallet datadir.

[Rust installation page]: https://www.rust-lang.org/tools/install
[crates.io]: https://crates.io

### Installing the latest development version

If you want to run the latest unpublished changes, then you can instead install Zallet
directly from the main branch of its code repository:

```
cargo install --locked --git https://github.com/zcash/wallet.git
```
