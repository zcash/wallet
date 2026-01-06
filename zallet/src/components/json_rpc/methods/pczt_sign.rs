//! PCZT sign method - sign a PCZT with wallet keys.

use documented::Documented;
use jsonrpsee::core::RpcResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::components::{
    database::DbHandle,
    json_rpc::server::LegacyCode,
    keystore::KeyStore,
};

pub(crate) type Response = RpcResult<ResultType>;

/// Result of signing a PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct SignResult {
    /// The base64-encoded signed PCZT.
    pub pczt: String,
    /// Number of transparent inputs signed.
    pub transparent_signed: usize,
    /// Number of Sapling spends signed.
    pub sapling_signed: usize,
    /// Number of Orchard actions signed.
    pub orchard_signed: usize,
}

pub(crate) type ResultType = SignResult;

/// Parameters for the pczt_sign RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct SignParams {
    /// The base64-encoded PCZT to sign.
    pub pczt: String,
    /// The account UUID whose keys should sign.
    pub account_uuid: Option<String>,
}

pub(super) const PARAM_PCZT_DESC: &str = "The base64-encoded PCZT to sign.";
pub(super) const PARAM_ACCOUNT_UUID_DESC: &str = "The account UUID whose keys should sign.";

/// Signs a PCZT with the wallet's keys.
pub(crate) async fn call(
    _wallet: DbHandle,
    _keystore: KeyStore,
    _pczt: &str,
    _account_uuid: Option<String>,
) -> Response {
    Err(LegacyCode::Misc.with_static(
        "pczt_sign is not yet implemented"
    ))
}
