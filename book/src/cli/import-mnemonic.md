# The `import-mnemonic` command

`zallet import-mnemonic` enables a [BIP 39] mnemonic to be imported into a Zallet wallet.

The command takes no arguments (beyond the top-level flags on `zallet` itself). When run,
Zallet will ask you to enter the mnemonic. It is recommended to paste the mnemonic in from
e.g. a password manager, as what you type will not be printed to the screen and thus it is
possible to make mistakes.

```
$ zallet import-mnemonic
Enter mnemonic:
```

Once the mnemonic has been provided, press Enter. Zallet will import the mnemonic, and
print out its ZIP 32 seed fingerprint (which you will use to identify it in other Zallet
commands and RPCs).

```
$ zallet import-mnemonic
Enter mnemonic:
Seed fingerprint: zip32seedfp1qhrfsdsqlj7xuvw3ncu76u98c2pxfyq2c24zdm5jr3pr6ms6dswss6dvur
```

[BIP 39]: https://github.com/bitcoin/bips/blob/master/bip-0039.mediawiki
