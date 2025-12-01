//! Implementation of the `z_shieldcoinbase` RPC meth
use std::future::Future;

use jsonrpsee::core::{JsonValue, RpcResult};
use zaino_state::FetchServiceSubscriber;

use crate::components::json_rpc::payments::SendResult;

use super::{ContextInfo, DbHandle, KeyStore, OperationId};

pub(crate) type ResultType = OperationId;
// TODO(schell): cargo culted from z_send_many - why do it this way?
pub(crate) type Response = RpcResult<ResultType>;

pub(crate) async fn call(
    mut wallet: DbHandle,
    keystore: KeyStore,
    chain: FetchServiceSubscriber,
    fromaddress: String,
    toaddress: String,
    fee: Option<JsonValue>,
    limit: Option<u32>,
    memo: Option<String>,
    privacy_policy: Option<String>,
) -> RpcResult<(
    Option<ContextInfo>,
    impl Future<Output = RpcResult<SendResult>>,
)> {
    todo!("meat of z_shieldcoinbase");
    Ok((
        Some(ContextInfo::new(
            "z_shieldcoinbase",
            serde_json::json!({
                "fromaddress": fromaddress,
                "toaddress": toaddress,
                "limit": limit,
            }),
        )),
        async { todo!() },
    ))
}
