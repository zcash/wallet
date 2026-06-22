use documented::Documented;
use jsonrpsee::core::RpcResult;
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::Serialize;
use zcash_client_backend::data_api::{Account as _, WalletRead};
use zcash_client_sqlite::AccountUuid;

use crate::components::{
    database::DbConnection,
    json_rpc::{server::LegacyCode, utils::ensure_wallet_is_unlocked},
    keystore::KeyStore,
};

/// Response to a `z_exportmnemonic` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = ExportMnemonic;

/// The exported mnemonic phrase.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct ExportMnemonic {
    /// The seed fingerprint of the exported mnemonic.
    seedfp: String,

    /// The BIP 39 mnemonic phrase, in plaintext.
    ///
    /// SECURITY: This is the wallet's most sensitive secret. Anyone with this phrase can
    /// spend all funds derived from it and recover the full transaction history. It is
    /// returned in the clear; handle it with care.
    mnemonic: String,
}

pub(super) const PARAM_ACCOUNT_UUID_DESC: &str =
    "The UUID of an account derived from the mnemonic phrase to export.";

pub(crate) async fn call(
    wallet: &DbConnection,
    keystore: &KeyStore,
    account_uuid: String,
) -> Response {
    let account_id = account_uuid
        .parse()
        .map(AccountUuid::from_uuid)
        .map_err(|_| {
            LegacyCode::InvalidParameter.with_message(format!("not a valid UUID: {account_uuid}"))
        })?;

    let account = wallet
        .get_account(account_id)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or_else(|| LegacyCode::InvalidAddressOrKey.with_static("Account not found"))?;

    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::Wallet.with_static("Account has no payment source (not derived from a seed)")
    })?;
    let seed_fp = derivation.seed_fingerprint();

    // Revealing the mnemonic requires the wallet to be unlocked.
    ensure_wallet_is_unlocked(keystore).await?;

    let mnemonic = keystore
        .reveal_mnemonic(seed_fp)
        .await
        .map_err(|e| LegacyCode::Wallet.with_message(e.to_string()))?;

    Ok(ExportMnemonic {
        seedfp: hex::encode(seed_fp.to_bytes()),
        mnemonic: mnemonic.expose_secret().to_string(),
    })
}
