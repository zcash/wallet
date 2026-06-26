//! A small direct JSON-RPC client for the validator (zebrad/zcashd), used by the
//! zebra-state backend for mempool access and transaction submission. Deliberately
//! Zaino-free: the `zaino-fetch` connector transitively pulls the full Zebra stack
//! (`zebra-state`, rocksdb), which would re-introduce the version coupling this backend
//! exists to remove.

use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use jsonrpsee::core::{client::ClientT, params::ArrayParams};
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};

use crate::error::{Error, ErrorKind};

enum Auth {
    /// Static Basic auth credentials (pre-encoded, never change).
    Basic(String),
    /// Cookie file path — re-read on every request so a zebrad restart is handled transparently.
    Cookie(PathBuf),
}

/// A JSON-RPC client for the backing validator.
#[derive(Clone)]
pub(crate) struct ValidatorRpcClient {
    url: String,
    auth: std::sync::Arc<Auth>,
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
        let auth = match cookie_path {
            Some(path) => Auth::Cookie(path.to_path_buf()),
            None => Auth::Basic(STANDARD.encode(format!("{user}:{password}"))),
        };

        Ok(Self {
            url: format!("http://{address}"),
            auth: std::sync::Arc::new(auth),
        })
    }

    /// Builds a fresh `HttpClient` with current credentials.
    ///
    /// For cookie auth this re-reads the cookie file, so a zebrad restart that
    /// rotates the cookie is handled transparently on the next request.
    fn build_client(&self) -> Result<HttpClient, Error> {
        let credentials = match self.auth.as_ref() {
            Auth::Basic(encoded) => encoded.clone(),
            Auth::Cookie(path) => {
                let content = std::fs::read_to_string(path)
                    .map_err(|e| ErrorKind::Generic.context(format!("reading RPC cookie: {e}")))?;
                let token = content
                    .trim()
                    .strip_prefix("__cookie__:")
                    .unwrap_or(content.trim());
                STANDARD.encode(format!("__cookie__:{token}"))
            }
        };

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Basic {credentials}"))
                .map_err(|e| ErrorKind::Generic.context(format!("invalid RPC auth header: {e}")))?,
        );

        HttpClientBuilder::default()
            .set_headers(headers)
            .build(&self.url)
            .map_err(|e| {
                ErrorKind::Generic
                    .context(format!("building RPC client: {e}"))
                    .into()
            })
    }

    /// `sendrawtransaction(hex)` — returns the txid hex string.
    pub(crate) async fn send_raw_transaction(&self, tx_hex: String) -> Result<String, Error> {
        let mut params = ArrayParams::new();
        params
            .insert(tx_hex)
            .map_err(|e| ErrorKind::Generic.context(e))?;
        self.build_client()?
            .request("sendrawtransaction", params)
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }

    /// `getrawmempool()` — returns the mempool txid hex strings.
    pub(crate) async fn get_raw_mempool(&self) -> Result<Vec<String>, Error> {
        self.build_client()?
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
        self.build_client()?
            .request("getrawtransaction", params)
            .await
            .map_err(|e| ErrorKind::Generic.context(e).into())
    }
}
