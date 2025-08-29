# The `rpc` command

> Available on **crate feature** `rpc-cli` only.

`zallet rpc` lets you communicate with a Zallet wallet's JSON-RPC interface from a
command-line shell.

- `zallet rpc help` will print a list of all JSON-RPC methods supported by Zallet.
- `zallet rpc help <method>` will print out a description of `<method>`.
- `zallet rpc <method>` will call that JSON-RPC method. Parameters can be provided via
  additional CLI arguments (`zallet rpc <method> <param>`).

## Comparison to `zcash-cli`

The `zcashd` full node came bundled with a `zcash-cli` binary, which served an equivalent
purpose to `zallet rpc`. There are some differences between the two, which we summarise
below:

| `zcash-cli` functionality         | `zallet rpc` equivalent            |
|-----------------------------------|------------------------------------|
| `zcash-cli -conf=<file>`          | `zallet --config <file> rpc`       |
| `zcash-cli -datadir=<dir>`        | `zallet --datadir <dir> rpc`       |
| `zcash-cli -stdin`                | Not implemented                    |
| `zcash-cli -rpcconnect=<ip>`      | `rpc.bind` setting in config file  |
| `zcash-cli -rpcport=<port>`       | `rpc.bind` setting in config file  |
| `zcash-cli -rpcwait`              | Not implemented                    |
| `zcash-cli -rpcuser=<user>`       | Not implemented                    |
| `zcash-cli -rpcpassword=<pw>`     | Not implemented                    |
| `zcash-cli -rpcclienttimeout=<n>` | `zallet rpc --timeout <n>`         |
| Hostname, domain, or IP address   | Only IP address                    |
| `zcash-cli <method> [<param> ..]` | `zallet rpc <method> [<param> ..]` |

For parameter parsing, `zallet rpc` is (as of the alpha releases) both more and less
flexible than `zcash-cli`:

- It is more flexible because `zcash-cli` implements type-checking on method parameters,
  which means that it cannot be used with Zallet JSON-RPC methods where the parameters
  have [changed](../zcashd/json_rpc.md). `zallet rpc` currently lacks this, which means
  that:
    - `zallet rpc` will work against both `zcashd` and `zallet` processes, which can be
      useful during the migration phase.
    - As the alpha and beta phases of Zallet progress, we can easily make changes to RPC
      methods as necessary.

- It is less flexible because parameters need to be valid JSON:
  - Strings need to be quoted in order to parse as JSON strings.
  - Parameters that contain strings need to be externally quoted.

| `zcash-cli` parameter | `zallet rpc` parameter |
|-----------------------|------------------------|
| `null`                | `null`                 |
| `true`                | `true`                 |
| `42`                  | `42`                   |
| `string`              | `'"string"'`           |
| `[42]`                | `[42]`                 |
| `["string"]`          | `'["string"]'`         |
| `{"key": <value>}`    | `'{"key": <value>}'`   |
