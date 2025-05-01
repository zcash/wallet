use std::collections::HashSet;

use documented::Documented;
use jsonrpsee::core::RpcResult;
use schemars::JsonSchema;
use serde::Serialize;

use crate::components::json_rpc::asyncop::{AsyncOperation, OperationState, OperationStatus};

/// Response to a `z_getoperationstatus` or `z_getoperationresult` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// The relevant operations.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
#[serde(transparent)]
pub(crate) struct ResultType(Vec<OperationStatus>);

/// Defines the method parameters for OpenRPC.
pub(super) fn params(g: &mut super::openrpc::Generator) -> Vec<super::openrpc::ContentDescriptor> {
    vec![g.param::<Vec<&str>>(
        "operationid",
        "A list of operation ids we are interested in.",
        false,
    )]
}

pub(crate) async fn status(async_ops: &[AsyncOperation], operationid: Vec<&str>) -> Response {
    let filter = operationid.into_iter().collect::<HashSet<_>>();

    let mut ret = vec![];

    for op in filtered(async_ops, filter) {
        ret.push(op.to_status().await);
    }

    Ok(ResultType(ret))
}

pub(crate) async fn result(
    async_ops: &mut Vec<AsyncOperation>,
    operationid: Vec<&str>,
) -> Response {
    let filter = operationid.into_iter().collect::<HashSet<_>>();

    let mut ret = vec![];
    let mut remove = HashSet::new();

    for op in filtered(async_ops, filter) {
        if matches!(
            op.state().await,
            OperationState::Success | OperationState::Failed | OperationState::Cancelled
        ) {
            ret.push(op.to_status().await);
            remove.insert(op.operation_id().to_string());
        }
    }

    async_ops.retain(|op| !remove.contains(op.operation_id()));

    Ok(ResultType(ret))
}

fn filtered<'a>(
    async_ops: &'a [AsyncOperation],
    filter: HashSet<&str>,
) -> impl Iterator<Item = &'a AsyncOperation> {
    async_ops
        .iter()
        .filter(move |op| filter.is_empty() || filter.contains(op.operation_id()))
}
