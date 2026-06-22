//! JSON-RPC server that is compatible with `zcashd`.

use std::net::SocketAddr;

use jsonrpsee::{
    server::{RpcServiceBuilder, Server},
    tracing::info,
};
use tokio::task::JoinHandle;

use crate::{
    components::{chain::Chain, database::Database},
    config::RpcSection,
    error::{Error, ErrorKind},
    fl,
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

pub(crate) async fn spawn<C: Chain>(
    config: RpcSection,
    wallet: Database,
    #[cfg(zallet_build = "wallet")] keystore: KeyStore,
    chain: C,
) -> Result<ServerTask, Error> {
    // Caller should make sure `bind` only contains a single address (for now).
    assert_eq!(config.bind.len(), 1);
    let listen_addr = config.bind[0];

    let timeout = config.timeout();
    let auth = authorization::AuthorizationLayer::new(config.auth)
        .map_err(|()| ErrorKind::Init.context(fl!("err-init-rpc-auth-invalid")))?;

    let (server_task, _addr) = spawn_inner(
        listen_addr,
        auth,
        timeout,
        wallet,
        #[cfg(zallet_build = "wallet")]
        keystore,
        chain,
    )
    .await?;

    Ok(server_task)
}

/// Spawns a JSON-RPC server bound to an ephemeral loopback port, authenticated with a
/// freshly-generated in-memory credential.
///
/// This is used by the in-process TUI frontend so that it can talk to the wallet over the
/// exact same JSON-RPC path used for remote connections, without requiring the user to
/// configure `rpc.bind`/`rpc.auth` and without exposing the port off-host or to disk.
///
/// Returns the spawned server task, the address the server actually bound to (the port is
/// chosen by the OS), and the credential the client must present.
#[cfg(feature = "tui")]
pub(crate) async fn spawn_ephemeral<C: Chain>(
    timeout: std::time::Duration,
    wallet: Database,
    #[cfg(zallet_build = "wallet")] keystore: KeyStore,
    chain: C,
) -> Result<(ServerTask, SocketAddr, super::EphemeralCredential), Error> {
    let credential = super::EphemeralCredential::generate();
    let auth = authorization::AuthorizationLayer::single(
        credential.user().to_string(),
        credential.password(),
    );

    // Bind to a loopback address with an OS-assigned port.
    let listen_addr: SocketAddr = (std::net::Ipv4Addr::LOCALHOST, 0).into();

    let (server_task, addr) = spawn_inner(
        listen_addr,
        auth,
        timeout,
        wallet,
        #[cfg(zallet_build = "wallet")]
        keystore,
        chain,
    )
    .await?;

    Ok((server_task, addr, credential))
}

/// Core server construction shared by [`spawn`] and `spawn_ephemeral`.
///
/// (`spawn_ephemeral` is only compiled with the `tui` feature, so it is not linked here.)
async fn spawn_inner<C: Chain>(
    listen_addr: SocketAddr,
    auth: authorization::AuthorizationLayer,
    timeout: std::time::Duration,
    wallet: Database,
    #[cfg(zallet_build = "wallet")] keystore: KeyStore,
    chain: C,
) -> Result<(ServerTask, SocketAddr), Error> {
    // Initialize the RPC methods.
    #[cfg(zallet_build = "wallet")]
    let wallet_rpc_impl = WalletRpcImpl::new(wallet.clone(), keystore.clone(), chain.clone());
    let rpc_impl = RpcImpl::new(
        wallet,
        #[cfg(zallet_build = "wallet")]
        keystore,
        chain,
    );

    let http_middleware = tower::ServiceBuilder::new()
        .layer(auth)
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

    Ok((server_task, addr))
}
