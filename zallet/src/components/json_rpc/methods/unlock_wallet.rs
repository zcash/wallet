use age::secrecy::SecretString;
use jsonrpsee::core::RpcResult;

use crate::components::{json_rpc::server::LegacyCode, keystore::KeyStore};

/// Response to a `walletpassphrase` RPC request.
pub(crate) type Response = RpcResult<()>;

pub(crate) async fn call(keystore: &KeyStore, passphrase: SecretString, timeout: u64) -> Response {
    if !keystore.is_crypted() {
        return Err(LegacyCode::WalletWrongEncState.with_static(
            "Error: running with an unencrypted wallet, but walletpassphrase was called.",
        ));
    }

    if !keystore.unlock(passphrase, timeout).await {
        return Err(LegacyCode::WalletPassphraseIncorrect
            .with_static("Error: The wallet passphrase entered was incorrect."));
    }

    Ok(())
}
