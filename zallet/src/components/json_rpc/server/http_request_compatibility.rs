//! Compatibility fixes for JSON-RPC HTTP requests.
//!
//! These fixes are applied at the HTTP level, before the RPC request is parsed.

use std::future::Future;
use std::pin::Pin;

use futures::FutureExt;
use http_body_util::BodyExt;
use hyper::{body::Bytes, header};
use jsonrpsee::{
    core::BoxError,
    server::{HttpBody, HttpRequest, HttpResponse},
};
use tower::Service;

/// HTTP [`HttpRequestMiddleware`] with compatibility workarounds.
///
/// This middleware makes the following changes to HTTP requests:
///
/// ### Map between the client's JSON-RPC version and JSON-RPC 2.0.
///
/// [`jsonrpsee`] only supports JSON-RPC 2.0, while the existing Zcash ecosystem is used
/// to communicating with `zcashd`'s "Bitcoin JSON-RPC" (a mix of 1.0, 1.1, and 2.0).
///
/// ### Add missing `content-type` HTTP header
///
/// Some RPC clients don't include a `content-type` HTTP header. But unlike web browsers,
/// [`jsonrpsee`] does not do content sniffing.
///
/// If there is no `content-type` header, we assume the content is JSON, and let the
/// parser error if we are incorrect.
///
/// ## Security
///
/// Any user-specified data in RPC requests is hex or base58check encoded. We assume the
/// client validates data encodings before sending it on to Zallet. So any fixes Zallet
/// performs won't change user-specified data.
#[derive(Clone, Debug)]
pub struct HttpRequestMiddleware<S> {
    service: S,
}

impl<S> HttpRequestMiddleware<S> {
    /// Create a new `HttpRequestMiddleware` with the given service.
    pub fn new(service: S) -> Self {
        Self { service }
    }

    /// Conditionally sets the `content-type` HTTP header to `application/json`.
    ///
    /// The header is inserted or replaced in the following cases:
    /// - no `content-type` supplied.
    /// - supplied `content-type` starts with `text/plain`, for example:
    ///   - `text/plain`
    ///   - `text/plain;`
    ///   - `text/plain; charset=utf-8`
    ///
    /// `application/json` is the only `content-type` accepted by the Zallet RPC endpoint,
    /// [as enforced by the `jsonrpsee` crate].
    ///
    /// [as enforced by the `jsonrpsee` crate]: https://github.com/paritytech/jsonrpsee/blob/656f8bb0793c8e992d20b47c3d17e7a6c396fb8b/server/src/transport/http.rs#L14-L29
    ///
    /// # Security
    ///
    /// - `content-type` headers exist so that applications know they are speaking the
    ///   correct protocol with the correct format. We can be a bit flexible, but there
    ///   are some types (such as binary) we shouldn't allow. In particular, the
    ///   `application/x-www-form-urlencoded` header should be rejected, so browser forms
    ///   can't be used to attack a local RPC port. This is handled by `jsonrpsee` as
    ///   mentioned above. See ["The Role of Routers in the CSRF Attack"].
    /// - Checking all the headers is secure, but only because `hyper` has custom code
    ///   that [just reads the first content-type header].
    ///
    /// ["The Role of Routers in the CSRF Attack"]: https://www.invicti.com/blog/web-security/importance-content-type-header-http-requests/
    /// [just reads the first content-type header]: https://github.com/hyperium/headers/blob/f01cc90cf8d601a716856bc9d29f47df92b779e4/src/common/content_type.rs#L102-L108
    pub fn insert_or_replace_content_type_header(headers: &mut header::HeaderMap) {
        if !headers.contains_key(header::CONTENT_TYPE)
            || headers
                .get(header::CONTENT_TYPE)
                .filter(|value| {
                    value
                        .to_str()
                        .ok()
                        .unwrap_or_default()
                        .starts_with("text/plain")
                })
                .is_some()
        {
            headers.insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static("application/json"),
            );
        }
    }

    /// Remove any "jsonrpc: 1.0" fields in `data`, and return the resulting string.
    pub fn remove_json_1_fields(data: String) -> String {
        // Replace "jsonrpc = 1.0":
        // - at the start or middle of a list, and
        // - at the end of a list;
        // with no spaces (lightwalletd format), and spaces after separators (example format).
        //
        // TODO: if we see errors from lightwalletd, make this replacement more accurate:
        //     - use a partial JSON fragment parser
        //     - combine the whole request into a single buffer, and use a JSON parser
        //     - use a regular expression
        //
        // We could also just handle the exact lightwalletd format,
        // by replacing `{"jsonrpc":"1.0",` with `{"jsonrpc":"2.0`.
        data.replace("\"jsonrpc\":\"1.0\",", "\"jsonrpc\":\"2.0\",")
            .replace("\"jsonrpc\": \"1.0\",", "\"jsonrpc\": \"2.0\",")
            .replace(",\"jsonrpc\":\"1.0\"", ",\"jsonrpc\":\"2.0\"")
            .replace(", \"jsonrpc\": \"1.0\"", ", \"jsonrpc\": \"2.0\"")
    }
}

/// Implements [`tower::Layer`] for [`HttpRequestMiddleware`].
#[derive(Clone)]
pub struct HttpRequestMiddlewareLayer {}

impl HttpRequestMiddlewareLayer {
    /// Creates a new `HttpRequestMiddlewareLayer`.
    pub fn new() -> Self {
        Self {}
    }
}

impl<S> tower::Layer<S> for HttpRequestMiddlewareLayer {
    type Service = HttpRequestMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        HttpRequestMiddleware::new(service)
    }
}

impl<S> Service<HttpRequest<HttpBody>> for HttpRequestMiddleware<S>
where
    S: Service<HttpRequest, Response = HttpResponse> + Clone + Send + 'static,
    S::Error: Into<BoxError> + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, mut request: HttpRequest<HttpBody>) -> Self::Future {
        // Fix the request headers.
        Self::insert_or_replace_content_type_header(request.headers_mut());

        let mut service = self.service.clone();
        let (parts, body) = request.into_parts();

        async move {
            let bytes = body
                .collect()
                .await
                .expect("Failed to collect body data")
                .to_bytes();

            let data = String::from_utf8_lossy(bytes.as_ref()).to_string();

            // Fix JSON-RPC 1.0 requests.
            let data = Self::remove_json_1_fields(data);
            let body = HttpBody::from(Bytes::from(data).as_ref().to_vec());

            let request = HttpRequest::from_parts(parts, body);

            service.call(request).await.map_err(Into::into)
        }
        .boxed()
    }
}
