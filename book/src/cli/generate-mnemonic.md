# The `generate-mnemonic` command

`zallet generate-mnemonic` generates a new [BIP 39] mnemonic and stores it in a Zallet
wallet.

The command takes no arguments (beyond the top-level flags on `zallet` itself). When run,
Zallet will generate a mnemonic, and add it to the wallet.

Each time you run `zallet generate-mnemonic`, a new mnemonic will be added to the wallet.
Be careful to only run it multiple times if you want multiple independent roots of spend
authority!

[BIP 39]: https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki
