use jsonrpsee::core::RpcResult;

use crate::components::{json_rpc::server::LegacyCode, keystore::KeyStore};

/// Response to a `walletlock` RPC request.
pub(crate) type Response = RpcResult<()>;

pub(crate) async fn call(keystore: &KeyStore) -> Response {
    if !keystore.uses_encrypted_identities() {
        return Err(LegacyCode::WalletWrongEncState
            .with_static("Error: running with an unencrypted wallet, but walletlock was called."));
    }

    keystore.lock().await;

    Ok(())
}
