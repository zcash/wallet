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
-allow-alpha-migration = --this-is-alpha-code-and-you-will-need-to-redo-the-migration-later

## Usage

usage-header = Usage

flags-header = Options

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

## zallet manpage

man-zallet-about = A {-zcash} wallet.

man-zallet-description =
    {-zallet} is a {-zcash} wallet.
