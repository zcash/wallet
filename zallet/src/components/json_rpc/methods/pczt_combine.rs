//! PCZT combine method - merge multiple PCZTs.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::combiner::Combiner;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pczt_decode::{MAX_PCZTS_TO_COMBINE, decode_pczt_base64};
use crate::components::json_rpc::server::LegacyCode;

pub(crate) type Response = RpcResult<ResultType>;

/// Result containing the combined PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct CombineResult {
    /// The base64-encoded combined PCZT.
    pub pczt: String,
}

pub(crate) type ResultType = CombineResult;

/// Parameters for the pczt_combine RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct CombineParams {
    /// An array of base64-encoded PCZTs to combine.
    pub pczts: Vec<String>,
}

pub(super) const PARAM_PCZTS_DESC: &str = "An array of base64-encoded PCZTs to combine.";
pub(super) const PARAM_PCZTS_REQUIRED: bool = true;

/// Combines multiple PCZTs into a single PCZT.
pub(crate) fn call(pczts_base64: Vec<String>) -> Response {
    if pczts_base64.is_empty() {
        return Err(LegacyCode::InvalidParameter.with_static("At least one PCZT is required"));
    }

    if pczts_base64.len() > MAX_PCZTS_TO_COMBINE {
        return Err(LegacyCode::InvalidParameter.with_message(format!(
            "Too many PCZTs to combine: {} exceeds maximum of {}",
            pczts_base64.len(),
            MAX_PCZTS_TO_COMBINE
        )));
    }

    let pczts = pczts_base64
        .iter()
        .enumerate()
        .map(|(i, pczt_base64)| {
            decode_pczt_base64(pczt_base64).map_err(|e| {
                LegacyCode::Deserialization
                    .with_message(format!("PCZT {i}: {}", e.message()))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let combiner = Combiner::new(pczts);
    let combined = combiner
        .combine()
        .map_err(|e| LegacyCode::Verify.with_message(format!("Failed to combine: {e:?}")))?;

    Ok(CombineResult { pczt: Base64::encode_string(&combined.serialize()) })
}
