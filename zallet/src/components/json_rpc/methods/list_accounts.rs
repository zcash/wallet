use documented::Documented;
use jsonrpsee::{core::RpcResult, types::ErrorCode as RpcErrorCode};
use schemars::JsonSchema;
use serde::Serialize;
use zcash_client_backend::{
    data_api::{Account as _, WalletRead},
    keys::UnifiedAddressRequest,
};

use crate::components::{database::DbConnection, json_rpc::server::LegacyCode};

/// Response to a `z_listaccounts` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// A list of accounts.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
#[serde(transparent)]
pub(crate) struct ResultType(Vec<Account>);

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct Account {
    /// The account's UUID within this Zallet instance.
    account_uuid: String,

    /// The ZIP 32 account ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<u64>,

    addresses: Vec<Address>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct Address {
    /// A diversifier index used in the account.
    diversifier_index: u128,

    /// The unified address corresponding to the diversifier.
    ua: String,
}

pub(crate) fn call(wallet: &DbConnection) -> Response {
    let mut accounts = vec![];

    for account_id in wallet
        .get_account_ids()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        let account = wallet
            .get_account(account_id)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            // This would be a race condition between this and account deletion.
            .ok_or(RpcErrorCode::InternalError)?;

        let address = wallet
            .get_last_generated_address_matching(account_id, UnifiedAddressRequest::ALLOW_ALL)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            // This would be a race condition between this and account deletion.
            .ok_or(RpcErrorCode::InternalError)?;

        // `z_listaccounts` assumes a single HD seed.
        // TODO: Fix this limitation.
        //       https://github.com/zcash/wallet/issues/82
        let account = account
            .source()
            .key_derivation()
            .map(|derivation| u32::from(derivation.account_index()).into());

        accounts.push(Account {
            account_uuid: account_id.expose_uuid().to_string(),
            account,
            addresses: vec![Address {
                // TODO: Expose the real diversifier index.
                //       https://github.com/zcash/wallet/issues/82
                diversifier_index: 0,
                ua: address.encode(wallet.params()),
            }],
        });
    }

    Ok(ResultType(accounts))
}
