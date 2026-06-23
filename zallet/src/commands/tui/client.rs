//! Typed JSON-RPC client for the TUI.
//!
//! This wraps a [`jsonrpsee`] HTTP client with one method per wallet RPC the TUI uses,
//! deserializing responses into purpose-built structs. The same client type is used for
//! both self-hosted (loopback) and remote connections.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use base64ct::{Base64, Encoding};
use hyper::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use jsonrpsee::core::{client::ClientT, params::ArrayParams};
use jsonrpsee_http_client::{HttpClient, HttpClientBuilder};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::json;

use crate::config::RpcAuthSection;

/// Errors that can arise when building or using the TUI's RPC client.
#[derive(Debug)]
pub(super) enum ClientError {
    Build(String),
    Request(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Build(e) => {
                write!(f, "{}", crate::fl!("tui-err-build-client", error = e))
            }
            ClientError::Request(e) => {
                write!(f, "{}", crate::fl!("tui-err-request", error = e))
            }
        }
    }
}

impl std::error::Error for ClientError {}

/// A JSON-RPC error returned by the wallet, including its numeric code.
///
/// The code matches the `zcashd`-compatible `LegacyCode` values (e.g. `-13` for
/// "wallet needs unlocking").
#[derive(Clone, Debug)]
pub(super) struct RpcError {
    pub(super) code: i32,
    pub(super) message: String,
}

impl RpcError {
    /// Whether this error indicates the wallet must be unlocked before the operation can
    /// proceed (`LegacyCode::WalletUnlockNeeded`).
    pub(super) fn is_unlock_needed(&self) -> bool {
        self.code == -13
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            crate::fl!(
                "tui-err-rpc-with-code",
                message = self.message.clone(),
                code = self.code
            )
        )
    }
}

/// The result of a wallet RPC call: either a typed value, or a structured wallet error.
pub(super) type CallResult<T> = Result<Result<T, RpcError>, ClientError>;

/// A typed JSON-RPC client for the wallet.
#[derive(Clone)]
pub(super) struct WalletClient {
    inner: HttpClient,
}

impl WalletClient {
    /// Connects to the self-hosted server at the given loopback address, authenticating
    /// with the RPC cookie credential (`user:password`) that the server generated on
    /// startup.
    ///
    /// The credential is sent via an HTTP `Authorization: Basic` header rather than being
    /// embedded in the URL, since the cookie password is Base64 and may contain characters
    /// that are not URL-safe.
    pub(super) fn connect_local(
        addr: SocketAddr,
        cookie: &SecretString,
        timeout: Duration,
    ) -> Result<Self, ClientError> {
        let encoded = Base64::encode_string(cookie.expose_secret().as_bytes());
        let mut value = HeaderValue::from_str(&format!("Basic {encoded}"))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        value.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, value);

        let inner = HttpClientBuilder::default()
            .request_timeout(timeout)
            .set_headers(headers)
            .build(format!("http://{addr}"))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Connects to a remote `zallet start` server at the given RPC URL.
    ///
    /// The URL may include a scheme (`http://` or `https://`); if it doesn't, `http://` is
    /// assumed (so a bare `host:port` works). If the URL does not already embed
    /// credentials, authentication is taken from the configured `[[rpc.auth]]` entries
    /// (the first entry with a cleartext password).
    pub(super) fn connect_remote(
        rpc_url: &str,
        auth: &[RpcAuthSection],
        timeout: Duration,
    ) -> Result<Self, ClientError> {
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

        Self::build(url.expose_secret(), timeout)
    }

    fn build(url: &str, timeout: Duration) -> Result<Self, ClientError> {
        let inner = HttpClientBuilder::default()
            .request_timeout(timeout)
            .build(url)
            .map_err(|e| ClientError::Build(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Performs a raw request, mapping a jsonrpsee error into either a structured
    /// [`RpcError`] (for wallet-level call errors) or a transport [`ClientError`].
    async fn request<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: ArrayParams,
    ) -> CallResult<T> {
        match self.inner.request::<T, _>(method, params).await {
            Ok(value) => Ok(Ok(value)),
            Err(jsonrpsee::core::client::Error::Call(err)) => Ok(Err(RpcError {
                code: err.code(),
                message: err.message().to_string(),
            })),
            Err(other) => Err(ClientError::Request(other.to_string())),
        }
    }

    // --- Status & balances ------------------------------------------------------------

    pub(super) async fn get_wallet_status(&self) -> CallResult<WalletStatus> {
        self.request("getwalletstatus", ArrayParams::new()).await
    }

    pub(super) async fn get_wallet_info(&self) -> CallResult<WalletInfo> {
        self.request("getwalletinfo", ArrayParams::new()).await
    }

    pub(super) async fn get_balances(&self, minconf: u32) -> CallResult<Balances> {
        let mut params = ArrayParams::new();
        params
            .insert(json!(minconf))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_getbalances", params).await
    }

    pub(super) async fn get_total_balance(&self, minconf: u32) -> CallResult<TotalBalance> {
        let mut params = ArrayParams::new();
        params
            .insert(json!(minconf))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        // `z_gettotalbalance` currently requires `include_watchonly = true`.
        params
            .insert(json!(true))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_gettotalbalance", params).await
    }

    // --- Accounts ---------------------------------------------------------------------

    pub(super) async fn list_accounts(&self) -> CallResult<Vec<Account>> {
        let mut params = ArrayParams::new();
        params
            .insert(json!(true))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_listaccounts", params).await
    }

    pub(super) async fn new_account(&self, name: &str) -> CallResult<serde_json::Value> {
        let mut params = ArrayParams::new();
        params
            .insert(json!(name))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_getnewaccount", params).await
    }

    pub(super) async fn new_address_for_account(
        &self,
        account_uuid: &str,
    ) -> CallResult<serde_json::Value> {
        let mut params = ArrayParams::new();
        params
            .insert(json!(account_uuid))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_getaddressforaccount", params).await
    }

    // --- Transactions -----------------------------------------------------------------

    pub(super) async fn list_transactions(
        &self,
        offset: u32,
        limit: u32,
    ) -> CallResult<Vec<WalletTx>> {
        let mut params = ArrayParams::new();
        // account_uuid, start_height, end_height: null (all)
        params
            .insert(serde_json::Value::Null)
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(serde_json::Value::Null)
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(serde_json::Value::Null)
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(json!(offset))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(json!(limit))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_listtransactions", params).await
    }

    // --- Wallet lock ------------------------------------------------------------------

    pub(super) async fn unlock(&self, passphrase: &str, timeout: u64) -> CallResult<()> {
        let mut params = ArrayParams::new();
        params
            .insert(json!(passphrase))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(json!(timeout))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        // `walletpassphrase` returns null on success.
        self.request("walletpassphrase", params).await
    }

    pub(super) async fn lock(&self) -> CallResult<()> {
        self.request("walletlock", ArrayParams::new()).await
    }

    // --- Sending ----------------------------------------------------------------------

    /// Submits a `z_sendmany`, returning the async operation id.
    pub(super) async fn send_many(
        &self,
        from: &str,
        to: &str,
        amount: &str,
        memo: Option<&str>,
        privacy_policy: &str,
    ) -> CallResult<String> {
        let mut recipient = serde_json::Map::new();
        recipient.insert("address".into(), json!(to));
        recipient.insert("amount".into(), json!(amount));
        if let Some(memo) = memo {
            recipient.insert("memo".into(), json!(memo));
        }

        let mut params = ArrayParams::new();
        params
            .insert(json!(from))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(json!([serde_json::Value::Object(recipient)]))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(json!(1)) // minconf
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(serde_json::Value::Null) // fee: ZIP-317 automatic
            .map_err(|e| ClientError::Build(e.to_string()))?;
        params
            .insert(json!(privacy_policy))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_sendmany", params).await
    }

    /// Polls the status of one async operation.
    pub(super) async fn operation_status(&self, opid: &str) -> CallResult<Vec<OperationStatus>> {
        let mut params = ArrayParams::new();
        params
            .insert(json!([opid]))
            .map_err(|e| ClientError::Build(e.to_string()))?;
        self.request("z_getoperationstatus", params).await
    }
}

// --- Response types -------------------------------------------------------------------
//
// These mirror the JSON shapes produced by the wallet RPC. They are intentionally
// tolerant: unknown fields are ignored, and optional fields are modelled as `Option`.

#[derive(Clone, Debug, Deserialize)]
pub(super) struct WalletStatus {
    pub(super) node_tip: ChainTip,
    #[serde(default)]
    pub(super) wallet_tip: Option<ChainTip>,
    #[serde(default)]
    pub(super) fully_synced_height: Option<u32>,
    #[serde(default)]
    pub(super) sync_work_remaining: Option<SyncWorkRemaining>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ChainTip {
    pub(super) blockhash: String,
    pub(super) height: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct SyncWorkRemaining {
    pub(super) unscanned_blocks: u32,
    pub(super) progress: Progress,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Progress {
    pub(super) numerator: u64,
    pub(super) denominator: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct TotalBalance {
    pub(super) transparent: String,
    pub(super) private: String,
    pub(super) total: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Balances {
    #[serde(default)]
    pub(super) accounts: Vec<AccountBalance>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct AccountBalance {
    pub(super) account_uuid: String,
    #[serde(default)]
    pub(super) transparent: Option<String>,
    #[serde(default)]
    pub(super) sapling: Option<String>,
    #[serde(default)]
    pub(super) orchard: Option<String>,
}

/// The wallet's encryption and lock state, derived from `getwalletinfo`.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct WalletInfo {
    /// The timestamp (seconds since epoch) until which the wallet is unlocked, or `0` if
    /// the wallet is encrypted but currently locked.
    ///
    /// This field is absent entirely when the wallet is unencrypted.
    #[serde(default)]
    pub(super) unlocked_until: Option<u64>,
}

impl WalletInfo {
    /// The wallet's encryption/lock state.
    pub(super) fn lock_state(&self) -> LockState {
        match self.unlocked_until {
            None => LockState::Unencrypted,
            Some(0) => LockState::Locked,
            Some(_) => LockState::Unlocked,
        }
    }
}

/// The encryption/lock state of the wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LockState {
    /// The wallet is not encrypted; there is no passphrase and nothing to unlock.
    Unencrypted,
    /// The wallet is encrypted and currently locked. Spending requires unlocking.
    Locked,
    /// The wallet is encrypted and currently unlocked.
    Unlocked,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Account {
    pub(super) account_uuid: String,
    #[serde(default)]
    pub(super) name: Option<String>,
    #[serde(default)]
    pub(super) addresses: Vec<AccountAddress>,
}

impl Account {
    /// A human-readable label for this account: its name, or a short UUID prefix.
    pub(super) fn label(&self) -> String {
        match &self.name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => {
                let short = if self.account_uuid.len() > 8 {
                    &self.account_uuid[..8]
                } else {
                    &self.account_uuid
                };
                format!("({short})")
            }
        }
    }

    /// Returns a unified address owned by this account, suitable for use as a `z_sendmany`
    /// source (`fromaddress`). `z_sendmany` does not accept account UUIDs, so the selected
    /// account must be resolved to one of its addresses.
    pub(super) fn spend_source_address(&self) -> Option<&str> {
        self.addresses
            .iter()
            .find_map(|a| a.ua.as_deref())
            .or_else(|| self.addresses.iter().find_map(|a| a.sapling.as_deref()))
            .or_else(|| self.addresses.iter().find_map(|a| a.transparent.as_deref()))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct AccountAddress {
    #[serde(default)]
    pub(super) ua: Option<String>,
    #[serde(default)]
    pub(super) sapling: Option<String>,
    #[serde(default)]
    pub(super) transparent: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct WalletTx {
    #[serde(default)]
    pub(super) account_uuid: Option<String>,
    #[serde(default)]
    pub(super) mined_height: Option<u32>,
    pub(super) txid: String,
    pub(super) account_balance_delta: i64,
    #[serde(default)]
    pub(super) fee_paid: Option<i64>,
    #[serde(default)]
    pub(super) block_time: Option<i64>,
    #[serde(default)]
    pub(super) expired_unmined: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct OperationStatus {
    pub(super) status: String,
    #[serde(default)]
    pub(super) error: Option<OperationError>,
    #[serde(default)]
    pub(super) result: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct OperationError {
    #[serde(default)]
    pub(super) message: Option<String>,
}
