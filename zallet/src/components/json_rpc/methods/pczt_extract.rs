//! PCZT extract method - extract final transaction from a completed PCZT.

use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::tx_extractor::TransactionExtractor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::pczt_decode::decode_pczt_base64;
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
///
/// The PCZT must have all required signatures and proofs in place.
/// 
/// NOTE: This method does not currently verify Sapling/Orchard proofs before
/// extraction. The resulting transaction will still be validated by the network
/// when broadcast.
pub(crate) fn call(pczt_base64: &str) -> Response {
    let pczt = decode_pczt_base64(pczt_base64)?;

    // NOTE: TransactionExtractor can optionally verify proofs with .with_sapling()
    // and .with_orchard() before extraction. For now we skip verification and let
    // the network validate the transaction on broadcast.
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
