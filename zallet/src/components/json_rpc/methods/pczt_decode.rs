//! PCZT decode method - decode and inspect a PCZT.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::Pczt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::components::json_rpc::server::LegacyCode;

use jsonrpsee::types::ErrorObjectOwned;

/// Maximum size for base64-encoded PCZT (10MB)
pub(super) const MAX_PCZT_BASE64_LEN: usize = 10 * 1024 * 1024;

/// Maximum number of PCZTs that can be combined in one call
pub(super) const MAX_PCZTS_TO_COMBINE: usize = 20;

/// Decode a base64-encoded PCZT with size limit check
pub(super) fn decode_pczt_base64(s: &str) -> Result<Pczt, ErrorObjectOwned> {
    if s.len() > MAX_PCZT_BASE64_LEN {
        return Err(LegacyCode::InvalidParameter.with_static("PCZT exceeds maximum size limit"));
    }
    let pczt_bytes = Base64::decode_vec(s).map_err(|e| {
        LegacyCode::Deserialization.with_message(format!("Invalid base64 encoding: {e}"))
    })?;
    Pczt::parse(&pczt_bytes)
        .map_err(|e| LegacyCode::Deserialization.with_message(format!("Invalid PCZT: {e:?}")))
}

pub(crate) type Response = RpcResult<ResultType>;

/// Decoded PCZT information.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct DecodedPczt {
    /// Transaction version.
    pub tx_version: u32,
    /// Version group ID.
    pub version_group_id: u32,
    /// Consensus branch ID.
    pub consensus_branch_id: u32,
    /// Expiry height.
    pub expiry_height: u32,
    /// Number of transparent inputs.
    pub transparent_inputs: usize,
    /// Number of transparent outputs.
    pub transparent_outputs: usize,
    /// Number of Sapling spends.
    pub sapling_spends: usize,
    /// Number of Sapling outputs.
    pub sapling_outputs: usize,
    /// Number of Orchard actions.
    pub orchard_actions: usize,
}

pub(crate) type ResultType = DecodedPczt;

/// Parameters for the pczt_decode RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct DecodeParams {
    /// The base64-encoded PCZT to decode.
    pub pczt: String,
}

pub(super) const PARAM_PCZT_DESC: &str = "The base64-encoded PCZT to decode.";

/// Decodes a PCZT and returns its structure.
pub(crate) fn call(pczt_base64: &str) -> Response {
    let pczt = decode_pczt_base64(pczt_base64)?;

    let global = pczt.global();

    Ok(DecodedPczt {
        tx_version: *global.tx_version(),
        version_group_id: *global.version_group_id(),
        consensus_branch_id: *global.consensus_branch_id(),
        expiry_height: *global.expiry_height(),
        transparent_inputs: pczt.transparent().inputs().len(),
        transparent_outputs: pczt.transparent().outputs().len(),
        sapling_spends: pczt.sapling().spends().len(),
        sapling_outputs: pczt.sapling().outputs().len(),
        orchard_actions: pczt.orchard().actions().len(),
    })
}
