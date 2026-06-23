//! `query` subcommand
//!
//! Constructs JSON-RPC requests from typed command-line arguments, rather than
//! requiring the caller to provide raw JSON parameters (as the `rpc` subcommand
//! does).
//!
//! Like the `tui` subcommand, the `query` command is fundamentally a JSON-RPC client and
//! supports two modes:
//!
//! - **Self-hosted (default):** boots the wallet backend in-process (database, keystore, and
//!   chain) and starts the JSON-RPC server on the configured loopback `rpc.bind` address.
//!   The server generates an RPC cookie in the data directory (see
//!   [`crate::components::json_rpc::server::cookie`]), which the command reads to
//!   authenticate over loopback. The data directory lock is held for the duration.
//! - **Remote (`--rpc-url`):** connects to an already-running `zallet start` instance using
//!   the `[[rpc.auth]]` credentials from the configuration.
//!
//! This mirrors the connection model used by `zallet tui`, reusing the same
//! [`server::spawn`] and [`cookie`] infrastructure, so there is a single tested path
//! regardless of where the server lives.
//!
//! Methods that require the wallet to be unlocked (e.g. `z_sendmany`) are handled
//! transparently: if the server reports that the wallet is locked, the command acquires the
//! passphrase (from `ZALLET_PASSPHRASE`, `--passphrase-stdin`, or an interactive prompt),
//! unlocks the wallet for a short window, runs the request, and re-locks afterwards.
//! Asynchronous operations (`z_sendmany`, `z_shieldcoinbase`) are polled to completion
//! unless `--no-wait` is given.

use std::fmt;
use std::time::Duration;

use abscissa_core::Runnable;
use base64ct::{Base64, Encoding};
use hyper::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use jsonrpsee::core::{client::ClientT, params::ArrayParams};
use jsonrpsee_http_client::{HttpClient, HttpClientBuilder};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value as JsonValue;

use crate::{
    cli::{QueryCmd, QueryMethodCmd},
    commands::AsyncRunnable,
    components::{
        chain::ZainoChain,
        database::Database,
        json_rpc::server::{self, cookie},
    },
    config::RpcAuthSection,
    error::{Error, ErrorKind},
    fl,
    prelude::*,
};

#[cfg(zallet_build = "wallet")]
use crate::components::keystore::KeyStore;

const DEFAULT_HTTP_CLIENT_TIMEOUT: u64 = 900;

/// Environment variable from which the wallet passphrase is read, if set.
const PASSPHRASE_ENV: &str = "ZALLET_PASSPHRASE";

/// The `LegacyCode::WalletPassphraseIncorrect` JSON-RPC error code.
const WALLET_PASSPHRASE_INCORRECT: i32 = -14;

macro_rules! wfl {
    ($f:ident, $message_id:literal) => {
        write!($f, "{}", $crate::fl!($message_id))
    };

    ($f:ident, $message_id:literal, $($args:expr),* $(,)?) => {
        write!($f, "{}", $crate::fl!($message_id, $($args), *))
    };
}

/// The number of seconds the wallet is unlocked for while a single `query` invocation
/// completes a request that requires unlocking. Kept short to bound key exposure; the
/// command also explicitly re-locks the wallet (see [`QueryCmd::execute`]).
const UNLOCK_TIMEOUT_SECS: u64 = 60;

/// How often to poll the status of an asynchronous operation.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The `LegacyCode::WalletUnlockNeeded` JSON-RPC error code, indicating that the wallet must
/// be unlocked before the requested operation can proceed.
const WALLET_UNLOCK_NEEDED: i32 = -13;

impl AsyncRunnable for QueryCmd {
    async fn run(&self) -> Result<(), Error> {
        let timeout = Duration::from_secs(match self.timeout {
            Some(0) => u64::MAX,
            Some(timeout) => timeout,
            None => DEFAULT_HTTP_CLIENT_TIMEOUT,
        });

        if let Some(rpc_url) = &self.rpc_url {
            self.run_remote(rpc_url, timeout).await
        } else {
            self.run_self_hosted(timeout).await
        }
    }
}

impl QueryCmd {
    /// Connects to a remote `zallet start` instance and makes the request.
    async fn run_remote(&self, rpc_url: &str, timeout: Duration) -> Result<(), Error> {
        let config = APP.config();

        let client = connect_remote(rpc_url, &config.rpc.auth, timeout)?;

        self.execute(&client).await
    }

    /// Boots the wallet backend in-process, starts the JSON-RPC server on the configured
    /// loopback `rpc.bind` address, makes the request over loopback (authenticating with the
    /// RPC cookie the server generates), and tears everything down once the request
    /// completes.
    async fn run_self_hosted(&self, timeout: Duration) -> Result<(), Error> {
        let config = APP.config();
        let _lock = config.lock_datadir()?;

        // The self-hosted server binds to the configured `rpc.bind` address (a single
        // loopback address) and authenticates this command via the RPC cookie it generates.
        // A bind address is required; the data directory cookie is what makes this secure
        // without configured credentials.
        let rpc_addr = match config.rpc.bind.as_slice() {
            [addr] => *addr,
            [] => {
                return Err(ErrorKind::Init
                    .context(
                        "`zallet query` requires a single loopback `rpc.bind` address to be \
                         configured (it hosts a local JSON-RPC server). Set `rpc.bind` in your \
                         configuration, or use `--rpc-url` to connect to a running `zallet start`.",
                    )
                    .into());
            }
            _ => {
                return Err(ErrorKind::Init
                    .context("`zallet query` supports only a single `rpc.bind` address")
                    .into());
            }
        };

        let datadir = config.datadir().to_path_buf();

        let db = Database::open(&config).await?;
        #[cfg(zallet_build = "wallet")]
        let keystore = KeyStore::new(&config, db.clone())?;

        // Start monitoring the chain so that chain-dependent methods work.
        let (chain, chain_indexer_task) = ZainoChain::new(&config).await?;

        // Launch the JSON-RPC server (which generates the RPC cookie in the data directory).
        let server_task = server::spawn(
            config.rpc.clone(),
            datadir.clone(),
            db,
            #[cfg(zallet_build = "wallet")]
            keystore,
            chain,
        )
        .await?;

        // Connect to our own server and make the request, shutting the background tasks
        // down regardless of the outcome.
        let result = async {
            // Read the cookie the server just generated, and use it to authenticate.
            let cookie = SecretString::new(cookie::read_cookie(&datadir)?);
            let client = connect_local(rpc_addr, &cookie, timeout)?;

            self.execute(&client).await
        }
        .await;

        server_task.abort();
        chain_indexer_task.abort();

        result
    }

    /// Runs the requested method against an already-connected client, transparently
    /// unlocking the wallet if the method requires it, and waiting for asynchronous
    /// operations to finish.
    ///
    /// The flow is:
    /// 1. Send the request. If it succeeds (or fails for any reason other than the wallet
    ///    being locked), we're done — methods that don't need unlocking never reach the
    ///    unlock path.
    /// 2. If the server reports `WalletUnlockNeeded` (`-13`), acquire the passphrase, call
    ///    `walletpassphrase` to unlock for a short window, and retry the request once. The
    ///    wallet is then re-locked with `walletlock` regardless of the outcome (important
    ///    when talking to a long-running remote `zallet start`).
    /// 3. If the (final) response is an asynchronous operation id and `--no-wait` was not
    ///    given, poll the operation to completion and report its result.
    async fn execute(&self, client: &HttpClient) -> Result<(), Error> {
        let (method, params) = self.method.to_request()?;

        let response = match request(client, &method, params).await {
            Ok(response) => response,
            Err(QueryError::WalletLocked) => {
                // The wallet is locked; unlock it, retry once, then re-lock.
                let passphrase = self.acquire_passphrase()?;
                unlock(client, passphrase).await?;

                // Rebuild the params (they were consumed by the first attempt) and retry.
                let (_, params) = self.method.to_request()?;
                let result = request(client, &method, params).await;

                // Best-effort re-lock. This matters most for a long-running remote server;
                // for the self-hosted case the process exits immediately afterwards.
                if let Err(e) = lock(client).await {
                    warn!("Failed to re-lock the wallet after the request: {e}");
                }

                result?
            }
            Err(e) => return Err(e.into()),
        };

        // If the response is an asynchronous operation id, optionally wait for it.
        let response = match operation_id(&response) {
            Some(opid) if !self.no_wait => poll_operation(client, &opid).await?,
            _ => response,
        };

        print_response(&response);
        Ok(())
    }

    /// Acquires the wallet passphrase, used when a method requires the wallet to be
    /// unlocked.
    ///
    /// In priority order:
    /// 1. The `ZALLET_PASSPHRASE` environment variable, if set.
    /// 2. The first line of standard input, if `--passphrase-stdin` was given.
    /// 3. An interactive, non-echoing terminal prompt.
    fn acquire_passphrase(&self) -> Result<SecretString, QueryError> {
        if let Ok(passphrase) = std::env::var(PASSPHRASE_ENV) {
            return Ok(SecretString::from(passphrase));
        }

        if self.passphrase_stdin {
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|_| QueryError::PassphraseRead)?;
            // Strip a single trailing line ending (`\n` or `\r\n`), preserving any other
            // characters in the passphrase.
            let trimmed = line.strip_suffix('\n').unwrap_or(&line);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
            return Ok(SecretString::from(trimmed.to_owned()));
        }

        let passphrase = rpassword::prompt_password(fl!("query-passphrase-prompt"))
            .map_err(|_| QueryError::PassphraseRead)?;
        Ok(SecretString::from(passphrase))
    }
}

/// Connects to the self-hosted server at the given loopback address, authenticating with the
/// RPC cookie credential (`user:password`) that the server generated on startup.
///
/// The credential is sent via an HTTP `Authorization: Basic` header rather than being
/// embedded in the URL, since the cookie password is Base64 and may contain characters that
/// are not URL-safe. This mirrors `zallet tui`'s local connection.
fn connect_local(
    addr: std::net::SocketAddr,
    cookie: &SecretString,
    timeout: Duration,
) -> Result<HttpClient, QueryError> {
    let encoded = Base64::encode_string(cookie.expose_secret().as_bytes());
    let mut value = HeaderValue::from_str(&format!("Basic {encoded}"))
        .map_err(|_| QueryError::FailedToConnect)?;
    value.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, value);

    HttpClientBuilder::default()
        .request_timeout(timeout)
        .set_headers(headers)
        .build(format!("http://{addr}"))
        .map_err(|_| QueryError::FailedToConnect)
}

/// Connects to a remote `zallet start` server at the given RPC URL.
///
/// The URL may include a scheme (`http://` or `https://`); if it doesn't, `http://` is
/// assumed (so a bare `host:port` works). If the URL does not already embed credentials,
/// authentication is taken from the configured `[[rpc.auth]]` entries (the first entry with a
/// cleartext password). This mirrors `zallet tui`'s remote connection.
fn connect_remote(
    rpc_url: &str,
    auth: &[RpcAuthSection],
    timeout: Duration,
) -> Result<HttpClient, QueryError> {
    // Split off an explicit scheme, defaulting to http.
    let (scheme, rest) = match rpc_url.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("http", rpc_url),
    };

    // If the user already embedded credentials (`user:pass@host`), keep them as-is;
    // otherwise inject the configured auth.
    let url = if rest.contains('@') {
        SecretString::new(format!("{scheme}://{rest}"))
    } else {
        let auth_prefix = auth
            .iter()
            .find_map(|a| {
                a.password
                    .as_ref()
                    .map(|pw| SecretString::new(format!("{}:{}@", a.user, pw.expose_secret())))
            })
            .unwrap_or_else(|| SecretString::new(String::new()));
        SecretString::new(format!("{scheme}://{}{rest}", auth_prefix.expose_secret()))
    };

    HttpClientBuilder::default()
        .request_timeout(timeout)
        .build(url.expose_secret())
        .map_err(|_| QueryError::FailedToConnect)
}

/// Sends a single JSON-RPC request, returning the deserialized JSON response.
///
/// A `WalletUnlockNeeded` (`-13`) call error is mapped to [`QueryError::WalletLocked`] so
/// that callers can transparently unlock and retry; all other errors become
/// [`QueryError::RequestFailed`].
async fn request(
    client: &HttpClient,
    method: &str,
    params: ArrayParams,
) -> Result<JsonValue, QueryError> {
    match client.request::<JsonValue, _>(method, params).await {
        Ok(response) => Ok(response),
        Err(jsonrpsee::core::client::Error::Call(err)) if err.code() == WALLET_UNLOCK_NEEDED => {
            Err(QueryError::WalletLocked)
        }
        Err(e) => Err(QueryError::RequestFailed(e.to_string())),
    }
}

/// Unlocks the wallet for [`UNLOCK_TIMEOUT_SECS`] using `walletpassphrase`.
///
/// An incorrect passphrase (`-14`) is reported as [`QueryError::PassphraseIncorrect`].
async fn unlock(client: &HttpClient, passphrase: SecretString) -> Result<(), QueryError> {
    let mut params = ArrayParams::new();
    push(&mut params, "passphrase", passphrase.expose_secret())?;
    push(&mut params, "timeout", UNLOCK_TIMEOUT_SECS)?;

    match client
        .request::<JsonValue, _>("walletpassphrase", params)
        .await
    {
        Ok(_) => Ok(()),
        Err(jsonrpsee::core::client::Error::Call(err))
            if err.code() == WALLET_PASSPHRASE_INCORRECT =>
        {
            Err(QueryError::PassphraseIncorrect)
        }
        Err(e) => Err(QueryError::RequestFailed(e.to_string())),
    }
}

/// Locks the wallet using `walletlock`.
async fn lock(client: &HttpClient) -> Result<(), QueryError> {
    client
        .request::<JsonValue, _>("walletlock", ArrayParams::new())
        .await
        .map(|_| ())
        .map_err(|e| QueryError::RequestFailed(e.to_string()))
}

/// Returns the asynchronous operation id contained in a response, if any.
///
/// The async methods either return the operation id directly as a string (`z_sendmany`)
/// or embed it in an `opid` field of an object (`z_shieldcoinbase`). Operation ids always
/// have the form `opid-<uuid>`, so they cannot be confused with other string responses.
fn operation_id(response: &JsonValue) -> Option<String> {
    let is_opid = |s: &str| s.starts_with("opid-");
    match response {
        JsonValue::String(s) if is_opid(s) => Some(s.clone()),
        JsonValue::Object(map) => match map.get("opid") {
            Some(JsonValue::String(s)) if is_opid(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Polls an asynchronous operation until it reaches a terminal state, returning its result
/// on success or an error describing the failure.
async fn poll_operation(client: &HttpClient, opid: &str) -> Result<JsonValue, QueryError> {
    loop {
        let mut params = ArrayParams::new();
        push(&mut params, "operationid", [opid])?;
        let statuses: JsonValue = client
            .request("z_getoperationstatus", params)
            .await
            .map_err(|e| QueryError::RequestFailed(e.to_string()))?;

        // The response is an array of status objects; we requested exactly one.
        let status = statuses
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| QueryError::OperationFailed(opid.to_owned()))?;

        match status.get("status").and_then(JsonValue::as_str) {
            Some("success") => {
                return Ok(status.get("result").cloned().unwrap_or(JsonValue::Null));
            }
            Some("failed") => {
                let message = status
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or("operation failed")
                    .to_owned();
                return Err(QueryError::OperationError(message));
            }
            Some("cancelled") => return Err(QueryError::OperationCancelled),
            // "queued" or "executing": keep waiting.
            _ => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }
}

/// Prints a JSON-RPC response to stdout: bare strings are printed as-is, everything else is
/// pretty-printed.
fn print_response(response: &JsonValue) {
    match response {
        JsonValue::String(s) => print!("{s}"),
        _ => serde_json::to_writer_pretty(std::io::stdout(), response)
            .expect("response should be valid"),
    }
}

/// Inserts a value into `params`, mapping serialization failures to a
/// [`QueryError`].
fn push<T: serde::Serialize>(
    params: &mut ArrayParams,
    name: &str,
    value: T,
) -> Result<(), QueryError> {
    params
        .insert(value)
        .map_err(|_| QueryError::InvalidParameter(name.into()))
}

/// Parses a string argument as a JSON value, mapping failures to a
/// [`QueryError`].
fn parse_json(name: &str, raw: &str) -> Result<JsonValue, QueryError> {
    serde_json::from_str(raw).map_err(|_| QueryError::InvalidParameter(name.into()))
}

/// Builds the `amounts` array for `z_sendmany` from the parallel `--to`, `--amount`, and
/// `--memo` arguments.
///
/// The Nth recipient pairs the Nth `--to` with the Nth `--amount` (and the Nth `--memo`, if
/// any were given). At least one recipient is required, and the number of `--to` and
/// `--amount` values must match (as must `--memo`, if any are provided).
#[cfg(zallet_build = "wallet")]
fn build_recipients(
    to: &[String],
    amounts: &[String],
    memos: &[String],
) -> Result<Vec<JsonValue>, QueryError> {
    if to.is_empty() {
        return Err(QueryError::Recipients(
            "at least one recipient is required (use --to and --amount)".into(),
        ));
    }
    if to.len() != amounts.len() {
        return Err(QueryError::Recipients(format!(
            "the number of --to ({}) and --amount ({}) arguments must match",
            to.len(),
            amounts.len(),
        )));
    }
    if !memos.is_empty() && memos.len() != to.len() {
        return Err(QueryError::Recipients(format!(
            "the number of --memo ({}) arguments must match the number of recipients ({})",
            memos.len(),
            to.len(),
        )));
    }

    to.iter()
        .enumerate()
        .map(|(i, address)| {
            // The amount is a numeric value; parse the string so that e.g. `1.5` is sent as
            // a JSON number rather than a string.
            let amount: JsonValue = amounts[i]
                .parse()
                .map_err(|_| QueryError::InvalidParameter(format!("--amount '{}'", amounts[i])))?;

            let mut recipient = serde_json::Map::new();
            recipient.insert("address".into(), JsonValue::from(address.clone()));
            recipient.insert("amount".into(), amount);
            if let Some(memo) = memos.get(i) {
                recipient.insert("memo".into(), JsonValue::from(memo.clone()));
            }
            Ok(JsonValue::Object(recipient))
        })
        .collect()
}

impl QueryMethodCmd {
    /// Builds the JSON-RPC method name and positional parameters for this
    /// command.
    fn to_request(&self) -> Result<(String, ArrayParams), QueryError> {
        let mut params = ArrayParams::new();

        let method = match self {
            QueryMethodCmd::GetWalletStatus => "getwalletstatus",

            QueryMethodCmd::ZListAccounts { include_addresses } => {
                push(&mut params, "include_addresses", include_addresses)?;
                "z_listaccounts"
            }

            QueryMethodCmd::ZGetAccount { account_uuid } => {
                push(&mut params, "account_uuid", account_uuid)?;
                "z_getaccount"
            }

            QueryMethodCmd::ZGetAddressForAccount {
                account,
                receiver_types,
                diversifier_index,
            } => {
                // The `account` parameter accepts either a UUID (string) or a
                // legacy account number (integer); preserve numbers as JSON
                // numbers and everything else as JSON strings.
                let account = match account.parse::<u64>() {
                    Ok(n) => JsonValue::from(n),
                    Err(_) => JsonValue::from(account.clone()),
                };
                push(&mut params, "account", account)?;
                push(&mut params, "receiver_types", receiver_types)?;
                push(&mut params, "diversifier_index", diversifier_index)?;
                "z_getaddressforaccount"
            }

            QueryMethodCmd::ListAddresses => "listaddresses",

            QueryMethodCmd::ZListUnifiedReceivers { unified_address } => {
                push(&mut params, "unified_address", unified_address)?;
                "z_listunifiedreceivers"
            }

            QueryMethodCmd::ZListTransactions {
                account_uuid,
                start_height,
                end_height,
                offset,
                limit,
            } => {
                push(&mut params, "account_uuid", account_uuid)?;
                push(&mut params, "start_height", start_height)?;
                push(&mut params, "end_height", end_height)?;
                push(&mut params, "offset", offset)?;
                push(&mut params, "limit", limit)?;
                "z_listtransactions"
            }

            QueryMethodCmd::GetRawTransaction {
                txid,
                verbose,
                blockhash,
            } => {
                push(&mut params, "txid", txid)?;
                push(&mut params, "verbose", verbose)?;
                push(&mut params, "blockhash", blockhash)?;
                "getrawtransaction"
            }

            QueryMethodCmd::DecodeRawTransaction { hexstring } => {
                push(&mut params, "hexstring", hexstring)?;
                "decoderawtransaction"
            }

            QueryMethodCmd::ZViewTransaction { txid } => {
                push(&mut params, "txid", txid)?;
                "z_viewtransaction"
            }

            QueryMethodCmd::ValidateAddress { address } => {
                push(&mut params, "address", address)?;
                "validateaddress"
            }

            QueryMethodCmd::VerifyMessage {
                zcashaddress,
                signature,
                message,
            } => {
                push(&mut params, "zcashaddress", zcashaddress)?;
                push(&mut params, "signature", signature)?;
                push(&mut params, "message", message)?;
                "verifymessage"
            }

            QueryMethodCmd::ZConvertTex {
                transparent_address,
            } => {
                push(&mut params, "transparent_address", transparent_address)?;
                "z_converttex"
            }

            QueryMethodCmd::DecodeScript { hexstring } => {
                push(&mut params, "hexstring", hexstring)?;
                "decodescript"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::Help { command } => {
                push(&mut params, "command", command)?;
                "help"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZListOperationIds { status } => {
                push(&mut params, "status", status)?;
                "z_listoperationids"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZGetOperationStatus { operationid } => {
                push(&mut params, "operationid", operationid)?;
                "z_getoperationstatus"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZGetOperationResult { operationid } => {
                push(&mut params, "operationid", operationid)?;
                "z_getoperationresult"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::GetWalletInfo => "getwalletinfo",

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::WalletPassphrase {
                passphrase,
                timeout,
            } => {
                push(&mut params, "passphrase", passphrase)?;
                push(&mut params, "timeout", timeout)?;
                "walletpassphrase"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::WalletLock => "walletlock",

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZGetNewAccount {
                account_name,
                seedfp,
            } => {
                push(&mut params, "account_name", account_name)?;
                push(&mut params, "seedfp", seedfp)?;
                "z_getnewaccount"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZGetBalances { minconf } => {
                push(&mut params, "minconf", minconf)?;
                "z_getbalances"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZImportAddress {
                account,
                hex_data,
                rescan,
            } => {
                push(&mut params, "account", account)?;
                push(&mut params, "hex_data", hex_data)?;
                push(&mut params, "rescan", rescan)?;
                "z_importaddress"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZGetTotalBalance {
                minconf,
                include_watchonly,
            } => {
                push(&mut params, "minconf", minconf)?;
                push(&mut params, "include_watchonly", include_watchonly)?;
                "z_gettotalbalance"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZListUnspent {
                minconf,
                maxconf,
                include_watchonly,
                addresses,
                as_of_height,
            } => {
                push(&mut params, "minconf", minconf)?;
                push(&mut params, "maxconf", maxconf)?;
                push(&mut params, "include_watchonly", include_watchonly)?;
                push(&mut params, "addresses", addresses)?;
                push(&mut params, "as_of_height", as_of_height)?;
                "z_listunspent"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZGetNotesCount {
                minconf,
                as_of_height,
            } => {
                push(&mut params, "minconf", minconf)?;
                push(&mut params, "as_of_height", as_of_height)?;
                "z_getnotescount"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZRecoverAccounts {
                name,
                seedfp,
                zip32_account_index,
                birthday_height,
            } => {
                // The RPC takes an array of account objects; the CLI recovers one account
                // per invocation.
                let account = serde_json::json!({
                    "name": name,
                    "seedfp": seedfp,
                    "zip32_account_index": zip32_account_index,
                    "birthday_height": birthday_height,
                });
                push(&mut params, "accounts", [account])?;
                "z_recoveraccounts"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZSendMany {
                fromaddress,
                to,
                amounts,
                memos,
                minconf,
                fee,
                privacy_policy,
            } => {
                let amount_objects = build_recipients(to, amounts, memos)?;
                let fee = fee
                    .as_deref()
                    .map(|raw| parse_json("fee", raw))
                    .transpose()?;
                push(&mut params, "fromaddress", fromaddress)?;
                push(&mut params, "amounts", amount_objects)?;
                push(&mut params, "minconf", minconf)?;
                push(&mut params, "fee", fee)?;
                push(&mut params, "privacy_policy", privacy_policy)?;
                "z_sendmany"
            }

            #[cfg(zallet_build = "wallet")]
            QueryMethodCmd::ZShieldCoinbase {
                fromaddress,
                toaddress,
                fee,
                limit,
                memo,
                privacy_policy,
            } => {
                let fee = fee
                    .as_deref()
                    .map(|raw| parse_json("fee", raw))
                    .transpose()?;
                push(&mut params, "fromaddress", fromaddress)?;
                push(&mut params, "toaddress", toaddress)?;
                push(&mut params, "fee", fee)?;
                push(&mut params, "limit", limit)?;
                push(&mut params, "memo", memo)?;
                push(&mut params, "privacy_policy", privacy_policy)?;
                "z_shieldcoinbase"
            }
        };

        Ok((method.to_owned(), params))
    }
}

impl Runnable for QueryCmd {
    fn run(&self) {
        self.run_on_runtime();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QueryError {
    FailedToConnect,
    InvalidParameter(String),
    Recipients(String),
    RequestFailed(String),
    /// The wallet is locked. This is an internal control-flow signal used to trigger the
    /// unlock-and-retry path; it is not surfaced to the user.
    WalletLocked,
    PassphraseRead,
    PassphraseIncorrect,
    OperationError(String),
    OperationCancelled,
    OperationFailed(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FailedToConnect => wfl!(f, "err-query-cli-conn-failed"),
            Self::InvalidParameter(param) => {
                wfl!(f, "err-query-cli-invalid-param", parameter = param)
            }
            Self::Recipients(reason) => {
                wfl!(f, "err-query-cli-recipients", reason = reason)
            }
            Self::RequestFailed(e) => {
                wfl!(f, "err-query-cli-request-failed", error = e)
            }
            Self::WalletLocked => wfl!(f, "err-query-cli-wallet-locked"),
            Self::PassphraseRead => wfl!(f, "err-query-cli-passphrase-read"),
            Self::PassphraseIncorrect => wfl!(f, "err-query-cli-passphrase-incorrect"),
            Self::OperationError(e) => {
                wfl!(f, "err-query-cli-operation-failed", error = e)
            }
            Self::OperationCancelled => wfl!(f, "err-query-cli-operation-cancelled"),
            Self::OperationFailed(opid) => {
                wfl!(f, "err-query-cli-operation-missing", operationid = opid)
            }
        }
    }
}

impl std::error::Error for QueryError {}

#[cfg(test)]
mod tests {
    use jsonrpsee::core::traits::ToRpcParams;

    use super::*;

    /// Builds the request for a command and returns the method name alongside
    /// the serialized JSON parameters.
    fn request_for(cmd: QueryMethodCmd) -> (String, String) {
        let (method, params) = cmd.to_request().expect("request should be valid");
        let json = params
            .to_rpc_params()
            .expect("params should serialize")
            .map(|raw| raw.get().to_owned())
            .unwrap_or_default();
        (method, json)
    }

    #[test]
    fn no_argument_method() {
        let (method, params) = request_for(QueryMethodCmd::GetWalletStatus);
        assert_eq!(method, "getwalletstatus");
        // A method with no arguments produces no parameters at all.
        assert_eq!(params, "");
    }

    #[test]
    fn optional_arguments_serialize_positionally() {
        // Omitted optional arguments are serialized as JSON `null` so that
        // positional arguments after them line up correctly.
        let (method, params) = request_for(QueryMethodCmd::ZListTransactions {
            account_uuid: None,
            start_height: Some(10),
            end_height: None,
            offset: None,
            limit: Some(5),
        });
        assert_eq!(method, "z_listtransactions");
        assert_eq!(params, "[null,10,null,null,5]");
    }

    #[test]
    fn account_number_is_serialized_as_a_number() {
        let (method, params) = request_for(QueryMethodCmd::ZGetAddressForAccount {
            account: "3".into(),
            receiver_types: vec![],
            diversifier_index: None,
        });
        assert_eq!(method, "z_getaddressforaccount");
        assert_eq!(params, "[3,[],null]");
    }

    #[test]
    fn account_uuid_is_serialized_as_a_string() {
        let (method, params) = request_for(QueryMethodCmd::ZGetAddressForAccount {
            account: "a-uuid".into(),
            receiver_types: vec!["p2pkh".into(), "orchard".into()],
            diversifier_index: Some(7),
        });
        assert_eq!(method, "z_getaddressforaccount");
        assert_eq!(params, r#"["a-uuid",["p2pkh","orchard"],7]"#);
    }

    #[cfg(zallet_build = "wallet")]
    #[test]
    fn send_many_builds_recipient_object_from_flat_args() {
        let (method, params) = request_for(QueryMethodCmd::ZSendMany {
            fromaddress: "ANY_TADDR".into(),
            to: vec!["tdest".into()],
            amounts: vec!["1.5".into()],
            memos: vec![],
            minconf: None,
            fee: Some("null".into()),
            privacy_policy: None,
        });
        assert_eq!(method, "z_sendmany");
        // The flat `--to`/`--amount` are assembled into the JSON object the RPC expects,
        // with the amount preserved as a number.
        assert_eq!(
            params,
            r#"["ANY_TADDR",[{"address":"tdest","amount":1.5}],null,null,null]"#
        );
    }

    #[cfg(zallet_build = "wallet")]
    #[test]
    fn send_many_supports_multiple_recipients_with_memo() {
        let (_, params) = request_for(QueryMethodCmd::ZSendMany {
            fromaddress: "ANY_TADDR".into(),
            to: vec!["zdest".into(), "tdest".into()],
            amounts: vec!["1".into(), "0.25".into()],
            memos: vec!["abcd".into(), String::new()],
            minconf: Some(2),
            fee: None,
            privacy_policy: Some("AllowRevealedAmounts".into()),
        });
        assert_eq!(
            params,
            r#"["ANY_TADDR",[{"address":"zdest","amount":1,"memo":"abcd"},{"address":"tdest","amount":0.25,"memo":""}],2,null,"AllowRevealedAmounts"]"#
        );
    }

    #[cfg(zallet_build = "wallet")]
    #[test]
    fn send_many_rejects_mismatched_to_and_amount() {
        let err = (QueryMethodCmd::ZSendMany {
            fromaddress: "ANY_TADDR".into(),
            to: vec!["a".into(), "b".into()],
            amounts: vec!["1".into()],
            memos: vec![],
            minconf: None,
            fee: None,
            privacy_policy: None,
        })
        .to_request()
        .expect_err("mismatched --to/--amount counts should be rejected");
        assert!(matches!(err, QueryError::Recipients(_)));
    }

    #[cfg(zallet_build = "wallet")]
    #[test]
    fn recover_accounts_builds_account_object_from_flat_args() {
        let (method, params) = request_for(QueryMethodCmd::ZRecoverAccounts {
            name: "acct".into(),
            seedfp: "0f6d".into(),
            zip32_account_index: 0,
            birthday_height: 2_800_000,
        });
        assert_eq!(method, "z_recoveraccounts");
        assert_eq!(
            params,
            r#"[[{"name":"acct","seedfp":"0f6d","zip32_account_index":0,"birthday_height":2800000}]]"#
        );
    }

    #[test]
    fn operation_id_detects_bare_string() {
        // `z_sendmany` returns the operation id directly as a string.
        let response = serde_json::json!("opid-1234");
        assert_eq!(operation_id(&response), Some("opid-1234".to_owned()));
    }

    #[test]
    fn operation_id_detects_object_field() {
        // `z_shieldcoinbase` returns an object with an `opid` field.
        let response = serde_json::json!({
            "shieldingUTXOs": 3,
            "opid": "opid-abcd",
        });
        assert_eq!(operation_id(&response), Some("opid-abcd".to_owned()));
    }

    #[test]
    fn operation_id_ignores_non_opid_strings() {
        // A non-opid string result (e.g. from `validateaddress`) must not be polled.
        let response = serde_json::json!("t1VmmGiyjVNeCjxDZzg7vZmd99WyzVby9yC");
        assert_eq!(operation_id(&response), None);
    }

    #[test]
    fn operation_id_ignores_objects_without_opid() {
        let response = serde_json::json!({ "isvalid": true });
        assert_eq!(operation_id(&response), None);
    }
}
