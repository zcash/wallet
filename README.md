# `zcashd` wallet replacement

Work in progress replacement of the `zcashd` wallet.

## Security Warnings

These crates are under development and have not been fully reviewed.

## Setting up a Zallet instance

This process is currently unstable, very manual, and subject to change as we
make Zallet easier to use.

Zallet is not designed to be used as a Rust library; we give no guarantees about
any such usage.

### Create a config file

Pick a folder to use for Zallet, and create a `zallet.toml` file in it. You
currently need at least the following:

```toml
network = "main"
wallet_db = "/path/to/zallet/datadir/data.sqlite"

[builder]

[indexer]
validator_user = ".."
validator_password = ".."
db_path = "/path/to/zallet/datadir/zaino"

[keystore]
identity = "/path/to/zallet/datadir/identity.txt"

[limits]

[rpc]
bind = ["127.0.0.1:SOMEPORT"]
```

If you have an existing `zcash.conf`, you can use it as a starting point:
```
$ zallet migrate-zcash-conf --datadir /path/to/zcashd/datadir -o /path/to/zallet/datadir/zallet.toml
```

The remaining commands currently require providing the path to the config file
via the `-c/--config` flag.

### Initialize the wallet encryption

Zallet uses [age encryption](https://age-encryption.org/) to encrypt all key
material internally. Currently you can use two kinds of age identities, which
you can generate with [`rage`](https://github.com/str4d/rage):

- A plain identity file directly on disk:
  ```
  $ rage-keygen -o /path/to/zallet/datadir/identity.txt
  Public key: age1...
  ```

- A passphrase-encrypted identity file:
  ```
  $ rage -p -o /path/to/zallet/datadir/identity.txt <(rage-keygen)
  Public key: age1...
  Using an autogenerated passphrase:
      drip-lecture-engine-wood-play-business-blame-kitchen-month-combine
  ```

(age plugins will also be supported but currently are tricky to set up.)

Once you have created your identity file, initialize your Zallet wallet:
```
$ zallet -c /path/to/zallet/datadir/zallet.toml init-wallet-encryption
```

### Generate a mnemonic phrase

```
$ zallet -c /path/to/zallet/datadir/zallet.toml generate-mnemonic
```

Each time you run this, a new [BIP 39](https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki)
mnemonic will be added to the wallet. Be careful to only run it multiple times
if you want multiple independent roots of spend authority!

### Start Zallet

```
$ zallet -c /path/to/zallet/datadir/zallet.toml start
```

## License

All code in this workspace is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
