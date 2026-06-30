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

### Choosing a chain backend

Zallet supports two chain backends, selected at compile time:

| Backend | Default | Platform | Requires |
|---------|---------|----------|---------|
| `zebra-state` | Yes | Linux only | co-located `zebrad` with `indexer` feature + `[indexer.read_state_service]` config |
| `zaino` | No | Linux, macOS, Windows | co-located `zebrad` JSON-RPC endpoint |

The **`zebra-state` backend** is the default. It reads finalized chain state directly from
a co-located `zebrad`'s state database and is the recommended choice for production
mainnet use on Linux.

The **`zaino` backend** fetches chain data over JSON-RPC (and optionally reads state
directly when `[indexer.read_state_service]` is configured). It is the only backend that
supports regtest and runs on non-Linux platforms.

To build or install with the `zaino` backend, pass `--no-default-features --features zaino`
(along with any other features you need):

```
# Install from crates.io with the zaino backend
cargo install --locked --no-default-features --features zaino,rpc-cli zallet

# Install the latest development version with the zaino backend
cargo install --locked --git https://github.com/zcash/wallet.git \
  --no-default-features --features zaino,rpc-cli
```

> Note: the two backends are mutually exclusive. Adding `--features zaino` without
> `--no-default-features` will activate both at once and produce a compile error.
