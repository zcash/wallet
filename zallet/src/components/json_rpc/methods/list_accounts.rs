use jsonrpsee::{core::RpcResult, tracing::warn};
use serde::{Deserialize, Serialize};

/// Response to a `z_listaccounts` RPC request.
pub(crate) type Response = RpcResult<Vec<Account>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Account {
    /// The ZIP 32 account ID.
    account: u64,
    addresses: Vec<Address>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Address {
    /// A diversifier index used in the account.
    diversifier_index: u128,

    /// The unified address corresponding to the diversifier.
    ua: String,
}

pub(crate) fn call() -> Response {
    warn!("TODO: Implement z_listaccounts");

    Ok(vec![])
}
