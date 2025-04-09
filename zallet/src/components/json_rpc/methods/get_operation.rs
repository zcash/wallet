use std::collections::HashSet;

use jsonrpsee::core::RpcResult;

use crate::components::json_rpc::asyncop::{AsyncOperation, OperationState, OperationStatus};

/// Response to a `z_getoperationstatus` or `z_getoperationresult` RPC request.
pub(crate) type Response = RpcResult<Vec<OperationStatus>>;

pub(crate) async fn status(async_ops: &[AsyncOperation], operationid: Vec<&str>) -> Response {
    let filter = operationid.into_iter().collect::<HashSet<_>>();

    let mut ret = vec![];

    for op in async_ops {
        if !filter.is_empty() && !filter.contains(op.operation_id()) {
            continue;
        }

        ret.push(op.to_status().await);
    }

    Ok(ret)
}

pub(crate) async fn result(
    async_ops: &mut Vec<AsyncOperation>,
    operationid: Vec<&str>,
) -> Response {
    let filter = operationid.into_iter().collect::<HashSet<_>>();

    let mut ret = vec![];
    let mut remove = HashSet::new();

    for op in async_ops.iter() {
        if !filter.is_empty() && !filter.contains(op.operation_id()) {
            continue;
        }

        if matches!(
            op.state().await,
            OperationState::Success | OperationState::Failed | OperationState::Cancelled
        ) {
            ret.push(op.to_status().await);
            remove.insert(op.operation_id().to_string());
        }
    }

    async_ops.retain(|op| !remove.contains(op.operation_id()));

    Ok(ret)
}
