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

-systemd = systemd

-zallet-add-rpc-user = {-zallet} add-rpc-user

-allow-warnings = --allow-warnings
-allow-alpha-example = --this-is-alpha-code-and-you-will-need-to-recreate-the-example-later
-allow-alpha-migration = --this-is-alpha-code-and-you-will-need-to-redo-the-migration-later
-allow-multiple-wallet-imports = --allow-multiple-wallet-imports
-datadir = --datadir
-db_dump = db_dump
-zcashd_install_dir = --zcashd_install_dir

-legacy_pool_seed_fingerprint = legacy_pool_seed_fingerprint
-zallet_toml = zallet.toml

-cfg-rpc-auth = rpc.auth
-cfg-rpc-auth-password = rpc.auth.password
-cfg-rpc-auth-pwhash = rpc.auth.pwhash

## Usage

usage-header = Usage

flags-header = Options

## Command prompts & output

cmd-add-rpc-user-prompt = Enter password:
cmd-add-rpc-user-instructions = Add this to your {-zallet_toml} file:
cmd-seed-fingerprint = Seed fingerprint: {$seedfp}
cmd-import-mnemonic-prompt = Enter mnemonic:

cmd-generate-encryption-identity-public-key = Public key: {$pubkey}
cmd-generate-encryption-identity-written = Encryption identity written to {$path}
cmd-generate-encryption-identity-write-failed = Failed to write encryption identity to {$path}: {$error}
cmd-generate-encryption-identity-exists = An encryption identity already exists at {$path}; refusing to overwrite it to avoid irrecoverable key loss.
cmd-generate-encryption-identity-passphrase-prompt = Enter passphrase to encrypt the identity:
cmd-generate-encryption-identity-passphrase-confirm = Confirm passphrase:
cmd-generate-encryption-identity-passphrase-mismatch = Passphrases do not match

## Startup messages

warn-config-unused = Config option '{$option}' is not yet implemented in {-zallet}; ignoring its value.

rpc-bare-password-auth-info = Using '{-cfg-rpc-auth-password}' authorization
rpc-bare-password-auth-warn =
    The use of '{-cfg-rpc-auth-password}' is less secure, because credentials are
    configured in plain text. It is recommended that locally-run instances switch to
    cookie-based auth, or otherwise to use '{-cfg-rpc-auth-pwhash}' credentials generated with
    '{-zallet-add-rpc-user}'.
rpc-pwhash-auth-info = Using '{-cfg-rpc-auth-pwhash}' authorization

rpc-cookie-generated = Generated RPC authentication cookie {$path}
rpc-cookie-read-failed = Failed to read cookie file: {$error}
rpc-cookie-user-conflict = Configured user conflicts with cookie auth username, skipping cookie generation

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
    This command is not stable, and parts of your {-zcashd} data may not get
    migrated correctly. You will need to rerun this command again once {-zallet}
    is stable to migrate your config correctly. To confirm you are aware of
    this, use '{-allow-alpha-migration}'.

migrate-config-written = {-zallet} config written to {$conf}

migrate-wallet-legacy-seed-fp =
    Importing zcashd legacy account for seed fingerprint '{$seed_fp}'. If you wish to
    enable legacy zcashd semantics for wallet RPC methods, you should set
    '{-legacy_pool_seed_fingerprint}' to this value in '{-zallet_toml}'.",

## General errors

err-kind-generic = Error
err-kind-init = Failed to initialize {-zallet}
err-kind-chain = An error occurred while accessing chain data.
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
err-init-db-incompatible-alpha =
    This wallet database was created by an incompatible alpha version of {-zallet}.
    To use this {-zallet} release, start again with a fresh Zallet wallet or a
    new data directory.
err-init-db-invalid-zallet-version =
    The wallet database recorded an invalid {-zallet} version '{$version}':
    {$err}

err-init-identity-not-found = Encryption identity file could not be located at {$path}
err-init-identity-not-passphrase-encrypted = {$path} is not encrypted with a passphrase
err-init-path-not-utf8 = {$path} is not currently supported (not UTF-8)
err-init-identity-not-usable = Identity file at {$path} is not usable: {$error}
err-init-rpc-auth-invalid = Invalid '{-cfg-rpc-auth}' configuration

## Keystore errors

err-keystore-missing-recipients = The wallet has not been set up to store key material securely.
rec-keystore-missing-recipients = Have you run '{$init_cmd}'?
err-keystore-already-initialized = Keystore age recipients already initialized
err-wallet-locked = Wallet is locked

## Account errors

err-account-not-found = Account does not exist
err-account-no-payment-source = Account has no payment source.

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
err-migrate-wallet-multi-import-disabled =
    The {-allow-multiple-wallet-imports} flag must be provided to allow the
    import of more than a single {-zcashd} `wallet.dat` file into {-zallet}.
err-migrate-wallet-duplicate-import =
    The {-zcashd} wallet containing seed with fingerprint '{$seed_fp}' has
    already been imported.
err-migrate-wallet-bdb-parse =
    An error occurred in parsing the {-zcashd} wallet file at '{$path}': '{$err}'
err-migrate-wallet-db-dump-not-found =
    The {-db_dump} command line utility was not found. Either set the
    {-zcashd-install-dir} command-line argument to the local zcashd installation
    root (which must contain the `zcutil/bin/` utilities) or ensure that {-db_dump}
    is available on the system `$PATH`.
err-migrate-wallet-db-dump =
    An error occurred in extracting wallet data from '{$path}': '{$err}'
err-migrate-wallet-seed-absent =
    The {-zcashd} wallet file did not contain HD seed information. Wallets from
    prior to the Sapling network upgrade are not supported by this migration
    tool.
err-migrate-wallet-invalid-mnemonic =
    The {-zcashd} wallet file contained invalid mnemonic seed phrase data and
    may be corrupt: '{$err}'
err-migrate-wallet-key-decoding=
    The {-zcashd} wallet file contained invalid mnemonic transparent secret key
    data and may be corrupt: '{$err}'
err-migrate-wallet-key-data=
    The {-zcashd} wallet file contained invalid key data and may be corrupt:
    '{$err}'
err-migrate-wallet-network-mismatch =
    The {-zcashd} wallet being imported is for the '{$wallet_network}' network,
    but this {-zallet} instance is configured for '{$zallet_network}'
err-migrate-wallet-regtest =
    Migration of regtest wallets is not yet supported.
err-migrate-wallet-storage =
    An database error occurred in wallet migration. This is indicative of a
    programming error; please report the following error to (TBD): '{$err}'
err-migrate-wallet-invalid-chain-data =
    Invalid chain data was encountered in wallet migration. This is indicative of a
    programming error; please report the following error to (TBD): '{$err}'
err-migrate-wallet-key-decoding =
    An error occurred decoding key material: '{$err}'.
err-migrate-wallet-tx-fetch =
    An error occurred fetching transaction data: '{$err}'.
err-migrate-wallet-data-parse=
    An error occurred parsing zcashd wallet data: '{$err}'.
err-migrate-wallet-invalid-account-id =
    Error encountered in wallet migration: '{$account_id}' is not a valid ZIP
    32 account identifier.
err-migrate-wallet-all-unmined =
    All transactions in the wallet are unmined; cannot determine effective
    consensus branch ID for pre-v5 transactions.

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

## RPC method errors

err-rpc-convert-tex-invalid-address = Invalid address
err-rpc-convert-tex-not-p2pkh = Address is not a transparent p2pkh address

## RPC CLI errors

err-rpc-cli-conn-failed = Failed to connect to the Zallet wallet's JSON-RPC port.
err-rpc-cli-invalid-param = Invalid parameter '{$parameter}'
err-rpc-cli-no-server = No JSON-RPC port is available.
err-rpc-cli-request-failed = JSON-RPC request failed: {$error}

## zallet manpage

man-zallet-about = A {-zcash} wallet.

man-zallet-description =
    {-zallet} is a {-zcash} wallet.

## Interactive terminal UI (`zallet tui`)

# Terms (not to be localized)
-tui-rpc-url = --rpc-url

# View / tab titles
tui-view-dashboard = Dashboard
tui-view-accounts = Accounts
tui-view-balances = Balances
tui-view-receive = Receive
tui-view-transactions = Transactions
tui-view-send = Send
tui-view-logs = Logs

# Sync status labels
tui-sync-synced = Synced
tui-sync-syncing = Syncing…
tui-sync-percent = Sync {$percent}%
tui-sync-heights = ({$synced} / {$node})
tui-sync-tip = (tip {$node})
tui-sync-blocks-left = · {$remaining} blocks left

# Generic units / values
tui-amount-zec = {$amount} ZEC
tui-value-unknown = (unknown)
tui-value-unnamed = (unnamed)

# Lock state (title bar)
tui-lock-unencrypted = unencrypted
tui-lock-locked = LOCKED
tui-lock-unlocked = unlocked

# Toasts: refresh and generic
tui-toast-refreshed = Refreshed

# Toasts: RPC call failures (method is a protocol identifier)
tui-err-rpc-call = {$method}: {$error}

# Toasts and prompts: lock / unlock
tui-unlock-not-encrypted-prompt = This wallet is not encrypted; there is no passphrase to enter.
tui-unlock-not-encrypted = This wallet is not encrypted; nothing to unlock.
tui-prompt-unlock-title = Unlock wallet (passphrase)
tui-prompt-new-account-title = New account name
tui-toast-unlocked = Wallet unlocked for 5 minutes
tui-err-incorrect-passphrase = Incorrect passphrase.
tui-err-unlock-failed = Unlock failed: {$error}
tui-toast-locked = Wallet locked
tui-lock-not-encrypted = This wallet is not encrypted; there is nothing to lock.

# Toasts: wallet locked, action needs unlock
tui-err-locked-press-u-lower = Wallet is locked. Press 'u' to unlock first.
tui-err-locked-press-u-upper = Wallet is locked. Press 'U' to unlock first.

# Toasts: accounts
tui-err-account-name-empty = Account name cannot be empty
tui-toast-account-created = Created account '{$name}'

# Toasts: send operations
tui-toast-send-completed = Send completed
tui-toast-send-cancelled = Send cancelled
tui-err-send-failed = Send failed: {$error}
tui-err-unknown = unknown error

# Logs view (toasts / placeholders)
tui-logs-remote-toast = Logs are written by the remote node in {-tui-rpc-url} mode.
tui-err-read-log = Could not read log file: {$error}

# Client errors (Display impls)
tui-err-build-client = failed to build RPC client: {$error}
tui-err-request = RPC request failed: {$error}
tui-err-rpc-with-code = {$message} (code {$code})

# Prompt modal
tui-prompt-hint = Enter to confirm · Esc to cancel

# Locked screen
tui-locked-title = Wallet locked
tui-locked-line1 = This wallet is encrypted and locked.
tui-locked-line2 = You must unlock it before you can view balances, addresses,
tui-locked-line3 = transactions, or send funds.
tui-locked-hint = Press 'u' or Enter to unlock · 'q' to quit

# Header / footer
tui-header-title = {-zallet} · {$lock}
tui-footer-gated = [u]nlock  [q]uit
tui-footer-unlock = [U]nlock{" "}
tui-footer-lock = [L]ock{" "}
tui-footer-nav-tabs = [h/l]tab [Enter]open
tui-footer-nav-view = [Esc]tabs [Tab]switch
tui-footer-hint = [?]help [q]uit [r]efresh {$nav} {$lock}

# Help overlay
tui-help-nav-header = Navigation
tui-help-nav-esc = Esc               Move focus to the tab row
tui-help-nav-switch-tabs = h/l or ←/→        Switch tabs (when on tab row)
tui-help-nav-enter = Enter or j        Enter the focused view
tui-help-nav-switch-view = Tab / Shift-Tab   Switch view directly
tui-help-nav-jump = 1..7              Jump to a view
tui-help-nav-select = j/k or ↑/↓        Move selection within a view
tui-help-global-header = Global
tui-help-global-refresh = r                 Refresh data
tui-help-global-unlock = U                 Unlock wallet (encrypted)
tui-help-global-lock = L                 Lock wallet (encrypted)
tui-help-global-help = ?                 Toggle this help
tui-help-global-quit = q / Ctrl-C        Quit
tui-help-accounts = Accounts: n = new   ·   Receive: ←/→ account, a = derive address
tui-help-transactions = Transactions: [ / ] = page   ·   Send: Enter or i edits a field
tui-help-logs = Logs: j/k scroll, g/G top/bottom, R reload
tui-help-close = Press any key to close
tui-help-title = Help

# Dashboard view
tui-dash-status-title = Status
tui-dash-node-tip = Node tip
tui-dash-node-hash = Node hash
tui-dash-wallet-tip = Wallet tip
tui-dash-not-syncing = (not yet syncing)
tui-dash-fully-synced-to = Fully synced to
tui-dash-loading-status = Loading wallet status…
tui-dash-accounts = Accounts
tui-dash-sync-title = Sync
tui-dash-sync-progress = {$percent}%  ({$blocks} blocks remaining)
tui-dash-fully-synced = Fully synced
tui-dash-balance-title = Balance
tui-dash-balances-syncing = Balances are not available yet — the wallet is still syncing.
tui-dash-total = Total
tui-dash-shielded = Shielded (private)
tui-dash-transparent = Transparent
tui-dash-total-unavailable = Total balance unavailable (watch-only, or not yet synced).
tui-dash-minconf = minconf = {$minconf}

# Accounts view
tui-accounts-title = Accounts
tui-accounts-empty = No accounts yet. Press 'n' to create one (wallet must be unlocked).
tui-accounts-title-list = Accounts  ([n]ew)
tui-accounts-balance-transparent = t:{$amount}
tui-accounts-balance-sapling = s:{$amount}
tui-accounts-balance-orchard = o:{$amount}

# Addresses (Receive) view
tui-addr-no-account-selected = No account selected.
tui-addr-derived = Derived a new address
tui-addr-kind-unified = unified
tui-addr-kind-sapling = sapling
tui-addr-kind-transparent = transparent
tui-addr-no-accounts = (no accounts)
tui-addr-account-label = {" "}Account:{" "}
tui-addr-account-hint = (←/→ account · j/k address · a new)
tui-addr-empty = No addresses for this account yet. Press 'a' to derive one.
tui-addr-title = Addresses
tui-addr-receive-title = Receive
tui-addr-select = Select an address to view it.
tui-addr-qr-enlarge = (enlarge the window to show the QR code)
tui-addr-qr-too-long = (address too long to render as QR)

# Balances view
tui-bal-header-account = Account
tui-bal-header-transparent = Transparent
tui-bal-header-sapling = Sapling
tui-bal-header-orchard = Orchard
tui-bal-title = Balances  (minconf = {$minconf}  [+/-] to change)
tui-bal-syncing = Balances are not available yet — the wallet is still syncing.
tui-bal-empty = No balances to display.

# Transactions view
tui-tx-title = Transactions  (page {$page}, [ / ] to page · experimental)
tui-tx-empty = No transactions to display.
tui-tx-unmined = unmined
tui-tx-detail-title = Detail
tui-tx-field-txid = txid
tui-tx-field-height = height
tui-tx-field-delta = delta
tui-tx-field-fee = fee
tui-tx-field-block-time = block time
tui-tx-field-account = account
tui-tx-expired = expired (unmined)
tui-tx-select = Select a transaction.

# Send view
tui-send-cancelled = Send cancelled
tui-send-err-no-accounts = No accounts available to send from
tui-send-err-no-source = Select a source account
tui-send-err-no-spendable = Selected account has no spendable address
tui-send-err-recipient-required = Recipient address is required
tui-send-err-amount-required = Amount is required
tui-send-err-amount-nan = Amount must be a number
tui-send-err-no-source-selected = No source account selected
tui-send-submitted = Submitted (op {$opid})
tui-send-from = From
tui-send-to = To
tui-send-amount = Amount (ZEC)
tui-send-memo = Memo
tui-send-privacy-policy = Privacy policy
tui-send-review = [ Review & send ]
tui-send-no-spendable-suffix = (no spendable address)
tui-send-fees-note = Fees are computed automatically (ZIP-317).
tui-send-privacy-warning = ⚠ This policy reduces privacy. Only proceed if you understand the implications.
tui-send-hint-editing = EDITING — type to enter text · Enter/Esc to finish
tui-send-hint-text = ↑↓ move · Enter to edit this field · Esc to tabs
tui-send-hint-submit = ↑↓ move · Enter to review & send · Esc to tabs
tui-send-hint-select = ↑↓ move · ←/→ change selection · Esc to tabs
tui-send-title = Send
tui-send-operation-title = Operation
tui-send-confirm = Confirm send?
tui-send-confirm-summary = {$amount} ZEC from {$from} → {$to}
tui-send-confirm-hint = [y] yes   [n] no
tui-send-queued = queued
tui-send-operation = Operation {$opid}
tui-send-status = Status: {$status}…
tui-send-succeeded = Send succeeded
tui-send-txid = txid: {$txid}
tui-send-failed = Last send did not succeed (see footer).
tui-send-placeholder = Fill in the form and press Enter to review.

# Logs view
tui-logs-file-label = {" "}Log file:{" "}
tui-logs-remote = Logs are written by the remote node when using {-tui-rpc-url}.
tui-logs-title = Logs  ({$state}  ·  j/k scroll · g/G top/bottom · R reload)
tui-logs-following = following
tui-logs-scrolled = scrolled
