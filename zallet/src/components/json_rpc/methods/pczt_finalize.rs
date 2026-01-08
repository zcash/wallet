//! PCZT finalize method - prepare a PCZT for signing.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::io_finalizer::IoFinalizer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pczt_decode::decode_pczt_base64;
use crate::components::json_rpc::server::LegacyCode;

pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = FinalizeResult;

/// Parameters for the pczt_finalize RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct FinalizeParams {
    /// The base64-encoded PCZT to finalize.
    pub pczt: String,
}

/// Result of finalizing a PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct FinalizeResult {
    /// The base64-encoded finalized PCZT.
    pub pczt: String,
    /// Whether IO finalization succeeded.
    pub finalized: bool,
}

pub(super) const PARAM_PCZT_DESC: &str = "The base64-encoded PCZT to finalize.";

/// Finalizes a PCZT by running IO finalization.
pub(crate) fn call(pczt_base64: &str) -> Response {
    let pczt = decode_pczt_base64(pczt_base64)?;

    let io_finalizer = IoFinalizer::new(pczt);
    let pczt = io_finalizer.finalize_io().map_err(|e| {
        LegacyCode::Verify.with_message(format!("IO finalization failed: {e:?}"))
    })?;

    Ok(FinalizeResult {
        pczt: Base64::encode_string(&pczt.serialize()),
        finalized: true,
    })
}
