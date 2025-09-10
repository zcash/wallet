# The `repair truncate-wallet` command

If a Zallet wallet gets into an inconsistent state due to a reorg that it cannot handle
automatically, `zallet start` will shut down. If you encounter this situation, you can use
`zallet repair truncate-wallet` to roll back the state of the wallet to before the reorg
point, and then start the wallet again to catch back up to the current chain tip.

The command takes one argument: the maximum height that the wallet should know about after
truncation. Due to how Zallet represents its state internally, there may be heights that
the wallet cannot roll back to, in which case a lower height may be used. The actual
height used by `zallet repair truncate-wallet` is printed to standard output:

```
$ zallet repair truncate-wallet 3000000
2999500
```
