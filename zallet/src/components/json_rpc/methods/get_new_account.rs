use jsonrpsee::{core::RpcResult, types::ErrorCode as RpcErrorCode};
use serde::Serialize;
use zaino_state::fetch::FetchServiceSubscriber;
use zcash_client_backend::{
    data_api::{AccountBirthday, WalletRead, WalletWrite},
    proto::service::TreeState,
};
use zcash_protocol::consensus::{NetworkType, Parameters};
use zip32::fingerprint::SeedFingerprint;

use crate::components::{
    database::DbConnection,
    json_rpc::{server::LegacyCode, utils::ensure_wallet_is_unlocked},
    keystore::KeyStore,
};

/// Response to a `z_getnewaccount` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = Account;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Account {
    /// The new account's UUID within this Zallet instance.
    account_uuid: String,

    /// The new account's ZIP 32 account index.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<u64>,
}

pub(crate) async fn call(
    wallet: &mut DbConnection,
    keystore: &KeyStore,
    chain: FetchServiceSubscriber,
    account_name: &str,
    seedfp: Option<&str>,
) -> Response {
    ensure_wallet_is_unlocked(keystore).await?;
    // TODO: Ensure wallet is backed up.

    let seedfp = seedfp
        .map(|s| {
            let mut hash = [0; 32];
            hex::decode_to_slice(s, &mut hash)?;
            Ok(SeedFingerprint::from_bytes(hash))
        })
        .transpose()
        .map_err(|e: hex::FromHexError| {
            LegacyCode::InvalidParameter.with_message(format!("Invalid seed fingerprint: {e}"))
        })?;

    let birthday_height = wallet
        .chain_height()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or(LegacyCode::InWarmup.with_static("Wallet sync required"))?;

    let treestate = {
        let treestate = chain
            .fetcher
            .get_treestate(birthday_height.saturating_sub(1).to_string())
            .await
            .map_err(|_| RpcErrorCode::InternalError)?;

        TreeState {
            network: match wallet.params().network_type() {
                NetworkType::Main => "main".into(),
                NetworkType::Test => "test".into(),
                NetworkType::Regtest => "regtest".into(),
            },
            height: u64::try_from(treestate.height).map_err(|_| RpcErrorCode::InternalError)?,
            hash: treestate.hash,
            time: treestate.time,
            sapling_tree: treestate.sapling.inner().inner().clone(),
            orchard_tree: treestate.orchard.inner().inner().clone(),
        }
    };

    let birthday = AccountBirthday::from_treestate(treestate, None)
        .map_err(|_| RpcErrorCode::InternalError)?;

    let seed_fps = keystore
        .list_seed_fingerprints()
        .await
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    let seed_fp = match (seed_fps.len(), seedfp) {
        (0, _) => Err(LegacyCode::Wallet
            .with_static("Wallet does not contain any seeds to generate accounts with")),
        (1, None) => Ok(seed_fps.into_iter().next().expect("present")),
        (_, None) => Err(LegacyCode::InvalidParameter
            .with_static("Wallet has more than one seed; seedfp argument must be provided")),
        (_, Some(seedfp)) => seed_fps
            .into_iter()
            .find(|fp| fp == &seedfp)
            .ok_or_else(|| {
                LegacyCode::InvalidParameter
                    .with_static("seedfp does not match any seed in the wallet")
            }),
    }?;

    let seed = keystore
        .decrypt_seed(&seed_fp)
        .await
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    let (account_id, _usk) = wallet
        .create_account(account_name, &seed, &birthday, None)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    Ok(Account {
        account_uuid: account_id.expose_uuid().to_string(),
        // TODO: Should we ever set this in Zallet?
        account: None,
    })
}
