# The `add-rpc-user` command

`zallet add-rpc-user` produces a config entry that authorizes a user to access the
JSON-RPC interface.

The command takes the username as its only argument. When run, Zallet will ask you to
enter the password. It is recommended to paste the password in from e.g. a password
manager, as what you type will not be printed to the screen and thus it is possible to
make mistakes.

```
$ zallet add-rpc-user foobar
Enter password:
```

Once the password has been provided, press Enter. Zallet will hash the password and print
out the user entry that you need to add to your config file.

```
$ zallet add-rpc-user foobar
Enter password:
Add this to your zallet.toml file:

[[rpc.auth]]
user = "foobar"
pwhash = "9a7e65104358b82cdd88e39155a5c36f$5564cf1836aa589f99250d7ddc11826cbb66bf9a9ae2079d43c353b1feaec445"
```
