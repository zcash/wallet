# Copyright 2024 The Electric Coin Company
#
# Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
# http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
# <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
# option. This file may not be copied, modified, or distributed
# except according to those terms.

### Localization for strings in the zallet binary

## Terms (not to be localized)

-zcash = Zcash
-zallet = zallet
-zcashd = zcashd
-zebrad = zebrad

{-systemd} = systemd

-allow-warnings = --allow-warnings
-allow-alpha-example = --this-is-alpha-code-and-you-will-need-to-recreate-the-example-later
-allow-alpha-migration = --this-is-alpha-code-and-you-will-need-to-redo-the-migration-later
-datadir = --datadir

## Usage

usage-header = Usage

flags-header = Options

## zallet.toml example messages

example-alpha-code =
    This command is not stable. You will need to rerun this command again once {-zallet}
    is stable to migrate your config correctly. To confirm you are aware of this, use
    '{-allow-alpha-example}'.

## zcash.conf migration messages

migrate-warnings = Some {-zcashd} options are not supported by {-zallet}:
migrate-warn-daemon =
    {-zallet} does not support the {-zcashd} option '{$option}'; instead you should
    use {-systemd} or similar to manage {-zallet} as a background service.
migrate-warn-disablewallet =
    The {-zcashd} config file has '{$option}' enabled, meaning that this {-zcashd}
    node's wallet was not being used. Check that you do intend to migrate its
    configuration to {-zallet}.
migrate-warn-paytxfee = '{$option}' is set, but {-zallet} only supports ZIP 317 fees.
migrate-warn-rpcport =
    {-zcashd} used the same port for both node and wallet RPC methods. {-zallet}
    has its own port for wallet RPC methods separate from the underlying {-zebrad}
    node, so the '{$option}' setting is not being migrated. If you want to change
    the default {-zallet} port, set '{$port}' in the {$rpc} section of the {-zallet}
    config file.
migrate-warn-sprout-migration =
    {-zallet} does not support Sprout, so the Sprout-to-Sapling migration option
    '{$option}' will not be migrated over.
migrate-warn-cli-only =
    {-zcashd} supported configuring '{$option}' via both a CLI flag and a config
    file entry. {-zallet} does not support it as a config file entry; you will
    instead need to start {-zallet} with the CLI flag '{$flag}'.
migrate-warn-unsupported =
    {-zallet} does not support an equivalent of the {-zcashd} option '{$option}',
    so its configured value '{$value}' is not being migrated. If this option is
    required for your use case, please get in touch with the {-zcash} developers
    as soon as possible to discuss alternatives.

migrate-alpha-code =
    This command is not stable, and parts of your {-zcashd} config may not get
    migrated correctly. You will need to rerun this command again once {-zallet}
    is stable to migrate your config correctly. To confirm you are aware of
    this, use '{-allow-alpha-migration}'.

migrate-config-written = {-zallet} config written to {$conf}

## General errors

err-kind-generic = Error
err-kind-init = Failed to initialize {-zallet}
err-kind-sync = Failed to synchronize {-zallet}

err-init-cannot-find-home-dir =
    Cannot find home directory for the default datadir. Use '{-datadir}' to set
    the datadir directly.
err-init-failed-to-create-lockfile = Failed to create a lockfile at {$path}: {$error}
err-init-failed-to-read-lockfile = Failed to read lockfile at {$path}: {$error}
err-init-zallet-already-running =
    Cannot obtain a lock on data directory {$datadir}. {-zallet} is probably already running.

err-init-config-db-mismatch =
    The wallet database was created for network type {$db_network_type}, but the
    config is using network type {$config_network_type}.

# errors in migration of configuration data from the zcashd `zcash.conf` config file format

err-migrate-allow-warnings = To allow a migration with warnings, use '{-allow-warnings}'
err-migrate-duplicate-zcashd-option =
    {-zcashd} option '{$option}' does not support multiple values,
    but appears multiple times in {$conf}
    Remove or comment out any duplicates so that it is only set once,
    then re-run this command.
err-migrate-invalid-line = Invalid line '{$line}' in {$conf}
err-migrate-invalid-zcashd-option = Invalid value '{$value}' for {-zcashd} option '{$option}'
err-migrate-multiple-related-zcashd-options =
    {-zcashd} option '{$option}' collides with '{$prev}', but both appear in
    {$conf}
    Remove one of the conflicting options, then re-run this command.
err-migrate-unknown-zcashd-option = Unknown {-zcashd} option '{$option}'

# errors in migration of wallet data from the zcashd `wallet.dat` database format

err-failed-seed-fingerprinting =
    Zallet was unable to import invalid seed data, likely due to the seed having
    an invalid length.

err-ux-A = Did {-zallet} not do what you expected? Could the error be more useful?
err-ux-B = Tell us
# Put (len(A) - len(B) - 41) spaces here.
err-ux-C = {"                    "}

## Limit errors

err-excess-orchard-actions =
    Including {$count} Orchard {$kind} would exceed the current limit of
    {$limit} actions, which exists to prevent memory exhaustion. Restart with
    '{$config}' where {$bound} to allow the wallet to attempt to construct this
    transaction.

## Privacy policy errors

err-privpol-no-privacy-not-allowed =
    This transaction would have no privacy, which is not enabled by default.
    THIS WILL AFFECT YOUR PRIVACY. Resubmit with the '{$parameter}' parameter
    set to '{$policy}' if you wish to allow this transaction to proceed anyway.
err-privpol-linking-addrs-not-allowed =
    This transaction would spend transparent funds received by multiple unified
    addresses within the same account, which is not enabled by default because
    it will publicly link those addresses together.
err-privpol-fully-transparent-not-allowed =
    This transaction would both spend transparent funds and have transparent
    recipients or change, which is not enabled by default because it will
    publicly reveal transaction participants and amounts.
err-privpol-transparent-sender-not-allowed =
    This transaction would spend transparent funds, which is not enabled by
    default because it will publicly reveal transaction senders and amounts.
err-privpol-transparent-recipient-not-allowed =
    This transaction would have transparent recipients, which is not enabled by
    default because it will publicly reveal transaction recipients and amounts.
err-privpol-transparent-change-not-allowed =
    This transaction would have transparent change, which is not enabled by
    default because it will publicly reveal the change address and amounts.
err-privpol-revealing-amount-not-allowed =
    Could not send to the {$pool} shielded pool without spending non-{$pool}
    funds, which would reveal transaction amounts.
err-privpol-transparent-receiver-not-allowed =
    This transaction would send to a transparent receiver of a unified address,
    which is not enabled by default because it will publicly reveal transaction
    recipients and amounts.
err-privpol-revealing-receiver-amounts-not-allowed =
    Could not send to a shielded receiver of a unified address without spending
    funds from a different pool, which would reveal transaction amounts.
rec-privpol-privacy-weakening =
    THIS MAY AFFECT YOUR PRIVACY. Resubmit with the '{$parameter}' parameter set
    to '{$policy}' or weaker if you wish to allow this transaction to proceed
    anyway.

## RPC CLI errors

err-rpc-cli-conn-failed = Failed to connect to the Zallet wallet's JSON-RPC port.
err-rpc-cli-invalid-param = Invalid parameter '{$parameter}'
err-rpc-cli-no-server = No JSON-RPC port is available.
err-rpc-cli-request-failed = JSON-RPC request failed: {$error}

## zallet manpage

man-zallet-about = A {-zcash} wallet.

man-zallet-description =
    {-zallet} is a {-zcash} wallet.
