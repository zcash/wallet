//! JSON-RPC server that is compatible with `zcashd`.

use jsonrpsee::{
    server::{RpcServiceBuilder, Server},
    tracing::info,
};
use tokio::task::JoinHandle;

use crate::{
    config::RpcSection,
    error::{Error, ErrorKind},
};

use super::methods::{RpcImpl, RpcServer as _};

mod error;
mod http_request_compatibility;
mod rpc_call_compatibility;

type ServerTask = JoinHandle<Result<(), Error>>;

pub(crate) async fn spawn(config: RpcSection) -> Result<ServerTask, Error> {
    // Caller should make sure `bind` only contains a single address (for now).
    assert_eq!(config.bind.len(), 1);
    let listen_addr = config.bind[0];

    // Initialize the RPC methods.
    let rpc_impl = RpcImpl::new();

    let http_middleware_layer = http_request_compatibility::HttpRequestMiddlewareLayer::new();

    let http_middleware = tower::ServiceBuilder::new()
        .layer(http_middleware_layer)
        .timeout(config.timeout());

    let rpc_middleware = RpcServiceBuilder::new()
        .rpc_logger(1024)
        .layer_fn(rpc_call_compatibility::FixRpcResponseMiddleware::new);

    let server_instance = Server::builder()
        .http_only()
        .set_http_middleware(http_middleware)
        .set_rpc_middleware(rpc_middleware)
        .build(listen_addr)
        .await
        .map_err(|e| ErrorKind::Init.context(e))?;
    let addr = server_instance
        .local_addr()
        .map_err(|e| ErrorKind::Init.context(e))?;
    info!("Opened RPC endpoint at {}", addr);

    let rpc_module = rpc_impl.into_rpc();

    let server_task = tokio::spawn(async move {
        server_instance.start(rpc_module).stopped().await;
        Ok(())
    });

    Ok(server_task)
}
