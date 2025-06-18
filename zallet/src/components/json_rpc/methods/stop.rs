use documented::Documented;
use jsonrpsee::{core::RpcResult, types::ErrorCode as RpcErrorCode};
use nix::sys::signal;
use schemars::JsonSchema;
use serde::Serialize;
use zcash_protocol::consensus::{NetworkType, Parameters};

use crate::components::database::DbHandle;

/// Response to a `stop` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// The stop response.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
#[serde(transparent)]
pub(crate) struct ResultType(());

pub(crate) fn call(wallet: DbHandle) -> Response {
    #[cfg(not(target_os = "windows"))]
    match wallet.params().network_type() {
        NetworkType::Regtest => {
            match signal::raise(signal::SIGINT) {
                Ok(_) => Ok(ResultType(())),
                Err(_) => Err(RpcErrorCode::InternalError.into()),
            }
        },
        _ => Err(RpcErrorCode::MethodNotFound.into()),
    }
    #[cfg(target_os = "windows")]
    Err(RpcErrorCode::MethodNotFound.into())
}
