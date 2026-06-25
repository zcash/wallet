//! A small direct JSON-RPC client for the validator (zebrad/zcashd), used by the
//! zebra-state backend for mempool access and transaction submission. Deliberately
//! Zaino-free: the `zaino-fetch` connector transitively pulls the full Zebra stack
//! (`zebra-state`, rocksdb), which would re-introduce the version coupling this backend
//! exists to remove.

use std::path::Path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use jsonrpsee::core::{client::ClientT, params::ArrayParams};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};

use crate::error::{Error, ErrorKind};

/// A JSON-RPC client for the backing validator.
#[derive(Clone)]
pub(crate) struct ValidatorRpcClient {
    client: HttpClient,
}

impl ValidatorRpcClient {
    /// Builds an authenticated client. Uses cookie auth when `cookie_path` is set,
    /// otherwise HTTP Basic auth with `user`/`password`.
    pub(crate) fn new(
        address: &str,
        user: &str,
        password: &str,
        cookie_path: Option<&Path>,
    ) -> Result<Self, Error> {
        let credentials = match cookie_path {
            Some(path) => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| ErrorKind::Init.context(format!("reading RPC cookie: {e}")))?;
                let token = content
                    .trim()
                    .strip_prefix("__cookie__:")
                    .unwrap_or(content.trim());
                STANDARD.encode(format!("__cookie__:{token}"))
            }
            None => STANDARD.encode(format!("{user}:{password}")),
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Basic {credentials}"))
                .map_err(|e| ErrorKind::Init.context(format!("invalid RPC auth header: {e}")))?,
        );

        let client = HttpClientBuilder::default()
            .set_headers(headers)
            .build(format!("http://{address}"))
            .map_err(|e| ErrorKind::Init.context(format!("building RPC client: {e}")))?;

        Ok(Self { client })
    }

    /// `sendrawtransaction(hex)` — returns the txid hex string.
    pub(crate) async fn send_raw_transaction(&self, tx_hex: String) -> Result<String, Error> {
        let mut params = ArrayParams::new();
        params
            .insert(tx_hex)
            .map_err(|e| ErrorKind::Generic.context(e))?;
        self.client
            .request("sendrawtransaction", params)
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }

    /// `getrawmempool()` — returns the mempool txid hex strings.
    pub(crate) async fn get_raw_mempool(&self) -> Result<Vec<String>, Error> {
        self.client
            .request("getrawmempool", ArrayParams::new())
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }

    /// `getrawtransaction(txid, 0)` — returns the raw transaction hex string.
    pub(crate) async fn get_raw_transaction(&self, txid_hex: String) -> Result<String, Error> {
        let mut params = ArrayParams::new();
        params
            .insert(txid_hex)
            .map_err(|e| ErrorKind::Generic.context(e))?;
        params
            .insert(0u8)
            .map_err(|e| ErrorKind::Generic.context(e))?;
        self.client
            .request("getrawtransaction", params)
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }
}
