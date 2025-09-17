# The `start` command

`zallet start` starts a Zallet wallet!

The command takes no arguments (beyond the top-level flags on `zallet` itself). When run,
Zallet will connect to the backing full node (which must be running), start syncing, and
begin listening for JSON-RPC connections.

You can shut down a running Zallet wallet with Ctrl+C if `zallet` is in the foreground,
or (on Unix systems) by sending it the signal `SIGINT` or `SIGTERM`.
