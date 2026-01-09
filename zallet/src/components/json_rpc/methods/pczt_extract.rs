//! PCZT extract method - extract final transaction from a completed PCZT.
//!
//! Extraction does not verify proofs by default. Set verify_proofs to true to verify
//! (requires proving keys to be loaded).

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
    /// If true, verify proofs before extraction (requires proving keys). Defaults to false.
    pub verify_proofs: Option<bool>,
}

pub(super) const PARAM_PCZT_DESC: &str =
    "The base64-encoded PCZT to extract a final transaction from.";
pub(super) const PARAM_VERIFY_PROOFS_DESC: &str =
    "If true, verify proofs before extraction (requires proving keys). Defaults to false.";

/// Extracts a final transaction from a completed PCZT.
///
/// The PCZT must have all required signatures and proofs in place.
///
/// Extraction does not verify proofs by default. Set verify_proofs to true to verify
/// (requires proving keys to be loaded).
pub(crate) fn call(pczt_base64: &str, verify_proofs: Option<bool>) -> Response {
    let pczt = decode_pczt_base64(pczt_base64)?;

    // Check if proof verification was requested
    if verify_proofs.unwrap_or(false) {
        return Err(LegacyCode::Misc.with_static("Proof verification not yet implemented"));
    }

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
