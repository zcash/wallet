//! JSON-RPC server that is compatible with `zcashd`.

use jsonrpsee::{
    server::{RpcServiceBuilder, Server},
    tracing::info,
};
use tokio::task::JoinHandle;

use crate::{
    components::{chain_view::ChainView, database::Database},
    config::RpcSection,
    error::{Error, ErrorKind},
};

use super::methods::{RpcImpl, RpcServer as _};

#[cfg(zallet_build = "wallet")]
use {
    super::methods::{WalletRpcImpl, WalletRpcServer},
    crate::components::keystore::KeyStore,
};

mod error;
pub(crate) use error::LegacyCode;

pub(crate) mod authorization;
mod http_request_compatibility;
mod rpc_call_compatibility;

type ServerTask = JoinHandle<Result<(), Error>>;

pub(crate) async fn spawn(
    config: RpcSection,
    wallet: Database,
    #[cfg(zallet_build = "wallet")] keystore: KeyStore,
    chain_view: ChainView,
) -> Result<ServerTask, Error> {
    // Caller should make sure `bind` only contains a single address (for now).
    assert_eq!(config.bind.len(), 1);
    let listen_addr = config.bind[0];

    // Initialize the RPC methods.
    #[cfg(zallet_build = "wallet")]
    let wallet_rpc_impl = WalletRpcImpl::new(wallet.clone(), keystore.clone(), chain_view.clone());
    let rpc_impl = RpcImpl::new(
        wallet,
        #[cfg(zallet_build = "wallet")]
        keystore,
        chain_view,
    );

    let timeout = config.timeout();

    let http_middleware = tower::ServiceBuilder::new()
        .layer(
            authorization::AuthorizationLayer::new(config.auth)
                .map_err(|()| ErrorKind::Init.context("Invalid `rpc.auth` configuration"))?,
        )
        .layer(http_request_compatibility::HttpRequestMiddlewareLayer::new())
        .timeout(timeout);

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

    #[allow(unused_mut)]
    let mut rpc_module = rpc_impl.into_rpc();
    #[cfg(zallet_build = "wallet")]
    rpc_module
        .merge(wallet_rpc_impl.into_rpc())
        .map_err(|e| ErrorKind::Init.context(e))?;

    let server_task = crate::spawn!("JSON-RPC server", async move {
        server_instance.start(rpc_module).stopped().await;
        Ok(())
    });

    Ok(server_task)
}
