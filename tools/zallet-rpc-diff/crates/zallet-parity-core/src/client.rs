use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use serde_json::Value;
use crate::{Result, Error};

/// A high-level RPC client wrapper for parity testing.
#[derive(Clone)]
pub struct RpcClient {
    inner: HttpClient,
    url: String,
}

impl RpcClient {
    /// Creates a new RPC client for the given URL.
    pub fn new(url: &str) -> Result<Self> {
        let inner = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| Error::Transport(e.to_string()))?;
        
        Ok(Self {
            inner,
            url: url.to_string(),
        })
    }

    /// Performs a generic RPC call.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        // jsonrpsee's request method handles JSON-RPC 2.0 serialization/deserialization.
        // We use Value as the return type to preserve the raw JSON for comparison.
        self.inner
            .request::<Value, _>(method, vec![params])
            .await
            .map_err(Into::into)
    }

    /// Returns the URL of this client.
    pub fn url(&self) -> &str {
        &self.url
    }
}
