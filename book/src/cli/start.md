# The `start` command

`zallet start` starts a Zallet wallet!

The command takes no arguments (beyond the top-level flags on `zallet` itself). When run,
Zallet will connect to the backing full node (which must be running), start syncing, and
begin listening for JSON-RPC connections.

You can shut down a running Zallet wallet with Ctrl+C if `zallet` is in the foreground,
or (on Unix systems) by sending it the signal `SIGINT` or `SIGTERM`.

## Tuning history recovery

When Zallet syncs a wallet for the first time (or recovers history for newly imported
keys), it downloads and trial-decrypts historical blocks in batches. The maximum number
of blocks in a batch is controlled by the `recover_batch_size` option in the `[sync]`
section of the [configuration file]:

```toml
[sync]
# Download and scan up to 10,000 blocks per batch.
recover_batch_size = 10000
```

Larger batches improve scanning throughput, but increase peak memory usage: every block
in a batch is held in memory while it is downloaded and trial-decrypted. Mainnet blocks
can currently be up to 2 MiB each, so a batch of `N` blocks can require on the order of
`N × 2 MiB` of memory. The default of `1000` is conservative; operators running on
server-class hardware may wish to raise it to speed up initial sync.

[configuration file]: example-config.md
