//! PCZT combine method - merge multiple PCZTs.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::{roles::combiner::Combiner, Pczt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

    let pczts: Vec<Pczt> = pczts_base64
        .iter()
        .enumerate()
        .map(|(i, pczt_base64)| {
            let pczt_bytes = Base64::decode_vec(pczt_base64).map_err(|e| {
                LegacyCode::Deserialization
                    .with_message(format!("Invalid base64 in PCZT {i}: {e}"))
            })?;
            Pczt::parse(&pczt_bytes).map_err(|e| {
                LegacyCode::Deserialization.with_message(format!("Invalid PCZT {i}: {e:?}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let combiner = Combiner::new(pczts);
    let combined = combiner
        .combine()
        .map_err(|e| LegacyCode::Verify.with_message(format!("Failed to combine: {e:?}")))?;

    Ok(CombineResult { pczt: Base64::encode_string(&combined.serialize()) })
}
