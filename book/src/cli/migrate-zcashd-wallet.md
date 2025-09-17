# The `migrate-zcashd-wallet` command

> Available on **crate feature** `zcashd-import` only.

`zallet migrate-zcashd-wallet` migrates a `zcashd` wallet file (`wallet.dat`) to a Zallet
wallet (`wallet.db`).

[`zallet init-wallet-encryption`] must be run before this command.

The command requires at least one of the following two flag:

- `--path`: A path to a `zcashd` wallet file.
- `--zcashd-datadir`: A path to a `zcashd` datadir. If this is provided, then `--path` can
  be relative (or omitted, in which case the default filename `wallet.dat` will be used).

> For the Zallet alpha releases, the command also currently takes another required flag
> `--this-is-alpha-code-and-you-will-need-to-redo-the-migration-later`.

When run, Zallet will parse the `zcashd` wallet file, connect to the backing full node
(to obtain necessary chain information for setting up wallet birthdays), create Zallet
accounts corresponding to the structure of the `zcashd` wallet, and store the key material
in the Zallet wallet.

If the optional flag `--buffer-wallet-transactions` is provided, Zallet will also migrate
the transactions from the `zcashd` wallet file. If this flag is omitted, then Zallet will
only migrate key material (transactions will instead be recovered from chain when the
Zallet wallet [is started]).

[`zcashd`]: https://github.com/zcash/zcash
[`zallet init-wallet-encryption`]: init-wallet-encryption.md
[is started]: start.md
