use crate::{Error, Result};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use serde_json::Value;
use std::time::Duration;

/// Default per-request timeout for RPC calls.
/// Keeps the harness from hanging indefinitely on an unresponsive node.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A high-level RPC client wrapper for parity testing.
#[derive(Clone)]
pub struct RpcClient {
    inner: HttpClient,
    url: String,
}

impl RpcClient {
    /// Creates a new RPC client with the default 30-second per-request timeout.
    pub fn new(url: &str) -> Result<Self> {
        Self::with_timeout(url, DEFAULT_REQUEST_TIMEOUT)
    }

    /// Creates a new RPC client with an explicit per-request timeout.
    ///
    /// The timeout applies per RPC call and prevents the harness from
    /// hanging indefinitely on an unresponsive or slow node.
    pub fn with_timeout(url: &str, timeout: Duration) -> Result<Self> {
        let inner = HttpClientBuilder::default()
            .request_timeout(timeout)
            .build(url)
            .map_err(|e| Error::Transport(e.to_string()))?;

        Ok(Self {
            inner,
            url: url.to_string(),
        })
    }

    /// Performs a generic JSON-RPC 2.0 call and returns the raw result value.
    ///
    /// Returns `Err(Error::JsonRpc)` on RPC-level errors (including method-not-found),
    /// and `Err(Error::Transport)` on connection / timeout failures.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.inner
            .request::<Value, _>(method, vec![params])
            .await
            .map_err(Into::into)
    }

    /// Returns the URL this client connects to.
    pub fn url(&self) -> &str {
        &self.url
    }
}
