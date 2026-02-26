use jsonrpsee::core::JsonValue;

// Imports to work around deficiencies in the build script.
#[cfg(zallet_build = "wallet")]
use super::{super::asyncop::OperationId, recover_accounts, z_send_many};

// See `generate_rpc_help()` in `build.rs` for how this is generated.
include!(concat!(env!("OUT_DIR"), "/rpc_openrpc.rs"));

pub(crate) fn call() -> openrpsee::openrpc::Response {
    let mut generator = openrpsee::openrpc::Generator::new();

    let methods = METHODS
        .into_iter()
        .map(|(name, method)| method.generate(&mut generator, name))
        .collect();

    Ok(openrpsee::openrpc::OpenRpc {
        openrpc: "1.3.2",
        info: openrpsee::openrpc::Info {
            title: "Zallet",
            description: crate::build::PKG_DESCRIPTION,
            version: crate::build::PKG_VERSION,
        },
        methods,
        components: generator.into_components(),
    })
}
