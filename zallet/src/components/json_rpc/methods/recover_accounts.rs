use std::collections::HashMap;

use documented::Documented;
use jsonrpsee::{
    core::RpcResult,
    types::{ErrorCode as RpcErrorCode, ErrorObjectOwned},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zaino_state::FetchServiceSubscriber;
use zcash_client_backend::{
    data_api::{Account as _, AccountBirthday, WalletRead, WalletWrite},
    proto::service::TreeState,
};
use zcash_protocol::consensus::{BlockHeight, NetworkType, Parameters};

use crate::components::{
    database::DbConnection,
    json_rpc::{
        server::LegacyCode,
        utils::{ensure_wallet_is_unlocked, parse_seedfp_parameter},
    },
    keystore::KeyStore,
};

/// Response to a `z_recoveraccounts` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = Accounts;

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub(crate) struct AccountParameter<'a> {
    name: &'a str,
    seedfp: &'a str,
    zip32_account_index: u32,
    birthday_height: u32,
}

/// The list of recovered accounts.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct Accounts {
    accounts: Vec<Account>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct Account {
    /// The account's UUID within this Zallet instance.
    account_uuid: String,

    seedfp: String,

    /// The account's ZIP 32 account index.
    zip32_account_index: u32,
}

pub(super) const PARAM_ACCOUNTS_DESC: &str =
    "An array of JSON objects representing the accounts to recover.";
pub(super) const PARAM_ACCOUNTS_REQUIRED: bool = true;

pub(crate) async fn call(
    wallet: &mut DbConnection,
    keystore: &KeyStore,
    chain: FetchServiceSubscriber,
    accounts: Vec<AccountParameter<'_>>,
) -> Response {
    ensure_wallet_is_unlocked(keystore).await?;
    // TODO: Ensure wallet is backed up.
    //       https://github.com/zcash/wallet/issues/201

    let recover_until = wallet
        .chain_height()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or(LegacyCode::InWarmup.with_static("Wallet sync required"))?;

    // Prepare arguments for the wallet.
    let mut account_args = vec![];
    for account in accounts {
        let seed_fp = parse_seedfp_parameter(account.seedfp)?;

        let account_index =
            zip32::AccountId::try_from(account.zip32_account_index).map_err(|e| {
                LegacyCode::InvalidParameter
                    .with_message(format!("Invalid ZIP 32 account index: {e}"))
            })?;

        let birthday_height = BlockHeight::from_u32(account.birthday_height);
        let treestate_height = birthday_height.saturating_sub(1);

        let treestate = {
            let treestate = chain
                .fetcher
                .get_treestate(treestate_height.to_string())
                .await
                .map_err(|e| {
                    LegacyCode::InvalidParameter.with_message(format!(
                        "Failed to get treestate at height {treestate_height}: {e}"
                    ))
                })?;

            TreeState {
                network: match wallet.params().network_type() {
                    NetworkType::Main => "main".into(),
                    NetworkType::Test => "test".into(),
                    NetworkType::Regtest => "regtest".into(),
                },
                height: u64::try_from(treestate.height).map_err(|_| RpcErrorCode::InternalError)?,
                hash: treestate.hash,
                time: treestate.time,
                sapling_tree: treestate
                    .sapling
                    .commitments()
                    .final_state()
                    .as_ref()
                    .map(hex::encode)
                    .unwrap_or_default(),
                orchard_tree: treestate
                    .orchard
                    .commitments()
                    .final_state()
                    .as_ref()
                    .map(hex::encode)
                    .unwrap_or_default(),
            }
        };

        let birthday = AccountBirthday::from_treestate(treestate, Some(recover_until))
            .map_err(|_| RpcErrorCode::InternalError)?;

        account_args.push((account.name, seed_fp, account_index, birthday));
    }

    // Fetch the seeds for the given seed fingerprints.
    let mut seeds = HashMap::new();
    for (_, seed_fp, _, _) in &account_args {
        if !seeds.contains_key(seed_fp) {
            let seed = keystore
                .decrypt_seed(seed_fp)
                .await
                .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

            seeds.insert(*seed_fp, seed);
        }
    }

    // Import the accounts.
    let accounts = account_args
        .into_iter()
        .map(|(account_name, seed_fp, account_index, birthday)| {
            let seed = seeds.get(&seed_fp).expect("present");

            let (account, _usk) = wallet
                .import_account_hd(account_name, seed, account_index, &birthday, None)
                .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

            Ok::<_, ErrorObjectOwned>(Account {
                account_uuid: account.id().expose_uuid().to_string(),
                seedfp: seed_fp.to_string(),
                zip32_account_index: account_index.into(),
            })
        })
        .collect::<Result<_, _>>()?;

    Ok(Accounts { accounts })
}
