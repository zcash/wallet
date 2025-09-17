# The `migrate-zcash-conf` command

> Available on **crate feature** `zcashd-import` only.

`zallet migrate-zcash-conf` migrates a [`zcashd`] configuration file (`zcash.conf`) to an
equivalent Zallet [configuration file] (`zallet.toml`).

The command requires at least one of the following two flag:

- `--path`: A path to a `zcashd` configuration file.
- `--zcashd-datadir`: A path to a `zcashd` datadir. If this is provided, then `--path` can
  be relative (or omitted, in which case the default filename `zcash.conf` will be used).

> For the Zallet alpha releases, the command also currently takes another required flag
> `--this-is-alpha-code-and-you-will-need-to-redo-the-migration-later`.

When run, Zallet will parse the `zcashd` config file, and migrate its various options to
equivalent Zallet config options. Non-wallet options will be ignored, and wallet options
that cannot be migrated will cause a warning to be printed to stdout.

[`zcashd`]: https://github.com/zcash/zcash
[configuration file]: example-config.md
