# The `migrate-zcashd-wallet` command

> Available on **crate feature** `zcashd-import` only.

`zallet migrate-zcashd-wallet` migrates a `zcashd` wallet file (`wallet.dat`) to a Zallet
wallet (`wallet.db`).

[`zallet init-wallet-encryption`] must be run before this command. In addition,
the `db_dump` utility (provided either by global installation or a local
source-based `zcashd` installation) must be available. Note that you specifically
need the `db_dump` utility built for BDB version 6.2.23 for greatest reliability.

The command requires at least one of the following two flag:

- `--path`: A path to a `zcashd` wallet file.
- `--zcashd-datadir`: A path to a `zcashd` datadir. If this is provided, then `--path` can
  be relative (or omitted, in which case the default filename `wallet.dat` will be used).

Additional CLI arguments:
- `--zcashd-install-dir`: A path to a local `zcashd` installation directory,
  for source-based builds of `zcashd`. This is used to find the installed
  version of the `db_dump` utility, which is required for operation. If not
  specified, Zallet will attempt to find `db_dump` on the system path; however,
  it is recommended to use a `db_dump` provided via local `zcashd` installation
  to ensure version compatibility with the `wallet.dat` file.
- `--allow-multiple-wallet-imports`: An optional flag that must be set if a
  user wants to import keys and transactions from multiple `wallet.dat` files
  (not required for the first `wallet.dat` import.)
- `--buffer-wallet-transactions`: If set, Zallet will eagerly fetch transaction
  data from the chain as part of wallet migration instead of via ordinary chain
  sync. This may speed up wallet recovery, but requires all wallet transactions
  to be buffered in-memory which may cause out-of-memory errors for large
  wallets.
- `--allow-warnings`: If set, Zallet will ignore errors in parsing transactions
  extracted from the `wallet.dat` file. This can enable the import of key data
  from wallets that have been used on consensus forks of the Zcash chain.

> For the Zallet alpha releases, the command also currently takes another required flag
> `--this-is-alpha-code-and-you-will-need-to-redo-the-migration-later`.

When run, Zallet will parse the `zcashd` wallet file, connect to the backing
full node (to obtain necessary chain information for setting up wallet
birthdays), create Zallet accounts corresponding to the structure of the
`zcashd` wallet, and store the key material in the Zallet wallet. Parsing is
performed using the `db_dump` command-line utility, which must either be
present in the `zcutil/bin` directory of a `zcashd` source installation (as
specified via the `--zcashd-install-dir` argument), or avaliable on the system
`$PATH`.

[`zcashd`]: https://github.com/zcash/zcash
[`zallet init-wallet-encryption`]: init-wallet-encryption.md
[is started]: start.md
