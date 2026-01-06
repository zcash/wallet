//! PCZT fund method - create a funded PCZT from a transaction proposal.

use documented::Documented;
use jsonrpsee::core::RpcResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zaino_state::FetchServiceSubscriber;

use crate::components::{
    database::DbHandle,
    json_rpc::server::LegacyCode,
    keystore::KeyStore,
};

pub(crate) type Response = RpcResult<ResultType>;

/// Result of funding a PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct FundResult {
    /// The base64-encoded funded PCZT.
    pub pczt: String,
}

pub(crate) type ResultType = FundResult;

/// Amount parameter for recipients.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct AmountParam {
    /// Recipient address.
    pub address: String,
    /// Amount in ZEC.
    pub amount: serde_json::Value,
    /// Optional memo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

pub(super) const PARAM_PCZT_DESC: &str = "Existing base64-encoded PCZT to add to.";
pub(super) const PARAM_FROM_ADDRESS_DESC: &str = "The address to send funds from.";
pub(super) const PARAM_AMOUNTS_DESC: &str = "An array of recipient amounts.";
pub(super) const PARAM_AMOUNTS_REQUIRED: bool = true;
pub(super) const PARAM_MINCONF_DESC: &str = "Minimum confirmations for inputs.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str = "Privacy policy for the transaction.";

/// Creates a funded PCZT from a transaction proposal.
pub(crate) async fn call(
    _wallet: DbHandle,
    _keystore: KeyStore,
    _chain: FetchServiceSubscriber,
    _pczt: Option<String>,
    _from_address: String,
    _amounts: Vec<AmountParam>,
    _minconf: Option<u32>,
    _privacy_policy: Option<String>,
) -> Response {
    Err(LegacyCode::Misc.with_static(
        "pczt_fund is not yet implemented"
    ))
}
