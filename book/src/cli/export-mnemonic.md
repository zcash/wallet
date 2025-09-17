# The `export-mnemonic` command

`zallet export-mnemonic` enables a BIP 39 mnemonic to be exported from a Zallet wallet.

The command takes the UUID of the account for which the mnemonic should be exported. You
can obtain this from a running Zallet wallet with `zallet rpc z_listaccounts`.

The mnemonic is encrypted to the same `age` identity that the wallet uses to internally
encrypt key material. You can then use a tool like [`rage`] to decrypt the resulting
file.

```
$ zallet export-mnemonic --armor 514ab5f4-62bd-4d8c-94b5-23fa8d8d38c2 >mnemonic.age
$ echo mnemonic.age
-----BEGIN AGE ENCRYPTED FILE-----
...
-----END AGE ENCRYPTED FILE-----
$ rage -d -i path/to/encrypted-identity.txt mnemonic.age
some seed phrase ...
```

[`rage`](https://github.com/str4d/rage)
