use jsonrpsee::{core::RpcResult, tracing::warn, types::ErrorCode};
use serde::Serialize;

/// Response to a `z_listunifiedreceivers` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = ListUnifiedReceivers;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ListUnifiedReceivers {
    /// The legacy P2PKH transparent address.
    ///
    /// Omitted if `p2sh` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    p2pkh: Option<String>,

    /// The legacy P2SH transparent address.
    ///
    /// Omitted if `p2pkh` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    p2sh: Option<String>,

    /// The legacy Sapling address.
    #[serde(skip_serializing_if = "Option::is_none")]
    sapling: Option<String>,

    /// A single-receiver Unified Address containing the Orchard receiver.
    #[serde(skip_serializing_if = "Option::is_none")]
    orchard: Option<String>,
}

pub(crate) fn call(unified_address: &str) -> Response {
    warn!("TODO: Implement z_listunifiedreceivers({unified_address})");

    Err(ErrorCode::MethodNotFound.into())
}
