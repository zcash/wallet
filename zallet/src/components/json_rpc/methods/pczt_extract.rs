//! PCZT extract method - extract final transaction from a completed PCZT.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::{roles::tx_extractor::TransactionExtractor, Pczt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::components::json_rpc::server::LegacyCode;

pub(crate) type Response = RpcResult<ResultType>;

/// Result containing the extracted transaction.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct ExtractResult {
    /// The hex-encoded raw transaction.
    pub hex: String,
}

pub(crate) type ResultType = ExtractResult;

/// Parameter for the pczt_extract RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct ExtractParams {
    /// The base64-encoded PCZT to extract a transaction from.
    pub pczt: String,
}

pub(super) const PARAM_PCZT_DESC: &str =
    "The base64-encoded PCZT to extract a final transaction from.";

/// Extracts a final transaction from a completed PCZT.
pub(crate) fn call(pczt_base64: &str) -> Response {
    let pczt_bytes = Base64::decode_vec(pczt_base64).map_err(|e| {
        LegacyCode::Deserialization.with_message(format!("Invalid base64 encoding: {e}"))
    })?;

    let pczt = Pczt::parse(&pczt_bytes)
        .map_err(|e| LegacyCode::Deserialization.with_message(format!("Invalid PCZT: {e:?}")))?;

    let extractor = TransactionExtractor::new(pczt);

    let tx = extractor.extract().map_err(|e| {
        LegacyCode::Verify.with_message(format!("Failed to extract transaction: {e:?}"))
    })?;

    let mut tx_bytes = Vec::new();
    tx.write(&mut tx_bytes).map_err(|e| {
        LegacyCode::Deserialization.with_message(format!("Failed to serialize transaction: {e}"))
    })?;

    Ok(ExtractResult { hex: hex::encode(tx_bytes) })
}
