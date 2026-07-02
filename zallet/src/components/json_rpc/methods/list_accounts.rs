use documented::Documented;
use jsonrpsee::{core::RpcResult, types::ErrorCode as RpcErrorCode};
use schemars::JsonSchema;
use serde::Serialize;
use zcash_client_backend::data_api::{Account as _, AddressSource, WalletRead};
use zcash_client_sqlite::AccountUuid;

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

    /// The account name.
    ///
    /// Omitted if the account has no configured name.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,

    /// The account's seed fingerprint.
    ///
    /// Omitted if the account has no known derivation information.
    #[serde(skip_serializing_if = "Option::is_none")]
    seedfp: Option<String>,

    /// The account's ZIP 32 account index.
    ///
    /// Omitted if the account has no known derivation information.
    #[serde(skip_serializing_if = "Option::is_none")]
    zip32_account_index: Option<u32>,

    /// The account's ZIP 32 account index.
    ///
    /// Omitted if the account has no known derivation information.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<u32>,

    /// The addresses known to the wallet for this account.
    ///
    /// Omitted if `include_addresses` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    addresses: Option<Vec<Address>>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(super) struct Address {
    /// A diversifier index used in the account to derive this address.
    #[serde(skip_serializing_if = "Option::is_none")]
    diversifier_index: Option<u128>,

    /// The unified address.
    ///
    /// Omitted if this is not a unified address.
    #[serde(skip_serializing_if = "Option::is_none")]
    ua: Option<String>,

    /// The Sapling address.
    ///
    /// Omitted if this is not a Sapling address.
    #[serde(skip_serializing_if = "Option::is_none")]
    sapling: Option<String>,

    /// The transparent address.
    ///
    /// Omitted if this is not a transparent address.
    #[serde(skip_serializing_if = "Option::is_none")]
    transparent: Option<String>,
}

pub(super) const PARAM_INCLUDE_ADDRESSES_DESC: &str =
    "Also include the addresses known to the wallet for this account.";

pub(crate) fn call(wallet: &DbConnection, include_addresses: Option<bool>) -> Response {
    let include_addresses = include_addresses.unwrap_or(true);

    let accounts = wallet
        .get_account_ids()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .into_iter()
        .map(|account_id| {
            account_details(
                wallet,
                account_id,
                include_addresses,
                |name, seedfp, zip32_account_index, addresses| Account {
                    account_uuid: account_id.expose_uuid().to_string(),
                    name,
                    seedfp,
                    zip32_account_index,
                    account: zip32_account_index,
                    addresses,
                },
            )
        })
        .collect::<Result<_, _>>()?;

    Ok(ResultType(accounts))
}

pub(super) fn account_details<T>(
    wallet: &DbConnection,
    account_id: AccountUuid,
    include_addresses: bool,
    f: impl FnOnce(Option<String>, Option<String>, Option<u32>, Option<Vec<Address>>) -> T,
) -> RpcResult<T> {
    let account = wallet
        .get_account(account_id)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        // This would be a race condition between this and account deletion.
        .ok_or(RpcErrorCode::InternalError)?;

    let name = account.name().map(String::from);

    let derivation = account.source().key_derivation();
    let seedfp = derivation.map(|derivation| derivation.seed_fingerprint().to_string());
    let account = derivation.map(|derivation| u32::from(derivation.account_index()));

    let addresses = wallet
        .list_addresses(account_id)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

    let addresses = include_addresses.then(|| {
        addresses
            .into_iter()
            .map(|a| {
                let diversifier_index = match a.source() {
                    AddressSource::Derived {
                        diversifier_index, ..
                    } => Some(diversifier_index.into()),
                    #[cfg(feature = "transparent-key-import")]
                    AddressSource::Standalone => None,
                };
                let enc = a.address().to_zcash_address(wallet.params()).to_string();
                let (ua, sapling, transparent) = match a.address() {
                    zcash_keys::address::Address::Sapling(_) => (None, Some(enc), None),
                    zcash_keys::address::Address::Transparent(_) => (None, None, Some(enc)),
                    zcash_keys::address::Address::Unified(_) => (Some(enc), None, None),
                    zcash_keys::address::Address::Tex(_) => {
                        unreachable!("zcash_client_sqlite never stores these")
                    }
                };
                Address {
                    diversifier_index,
                    ua,
                    sapling,
                    transparent,
                }
            })
            .collect()
    });

    Ok(f(name, seedfp, account, addresses))
}

#[cfg(test)]
mod tests {
    mod integration {
        use zcash_protocol::consensus;

        use crate::{components::testing::TestWallet, network::Network};

        use super::super::*;

        /// Test z_listaccounts returns empty list for a wallet with no accounts.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn list_accounts_empty_wallet() {
            let network = Network::Consensus(consensus::Network::MainNetwork);
            let wallet = TestWallet::new(network).await.unwrap();

            let handle = wallet.handle().await.unwrap();

            let result = call(handle.as_ref(), Some(true));

            assert!(result.is_ok());
            let accounts = result.unwrap();
            assert!(accounts.0.is_empty(), "Expected empty account list");
        }

        /// Test z_listaccounts returns a single account correctly.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn list_accounts_single_account() {
            let network = Network::Consensus(consensus::Network::MainNetwork);
            let wallet = TestWallet::new(network).await.unwrap();

            let account = wallet
                .account_builder()
                .with_name("my_account")
                .build()
                .await
                .unwrap();

            let handle = wallet.handle().await.unwrap();

            let result = call(handle.as_ref(), Some(true));

            assert!(result.is_ok());
            let accounts = result.unwrap();
            assert_eq!(accounts.0.len(), 1, "Expected exactly one account");

            let listed = &accounts.0[0];
            assert_eq!(
                listed.account_uuid,
                account.account_id.expose_uuid().to_string()
            );
            assert_eq!(listed.name.as_deref(), Some("my_account"));
            assert!(listed.seedfp.is_some(), "Should have seed fingerprint");
        }

        /// Test z_listaccounts returns multiple accounts correctly.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn list_accounts_multiple_accounts() {
            let network = Network::Consensus(consensus::Network::MainNetwork);
            let wallet = TestWallet::new(network).await.unwrap();

            let account1 = wallet
                .account_builder()
                .with_name("first_account")
                .build()
                .await
                .unwrap();

            let account2 = wallet
                .account_builder()
                .with_name("second_account")
                .build()
                .await
                .unwrap();

            let handle = wallet.handle().await.unwrap();

            let result = call(handle.as_ref(), Some(true));

            assert!(result.is_ok());
            let accounts = result.unwrap();
            assert_eq!(accounts.0.len(), 2, "Expected exactly two accounts");

            // Verify both accounts are present (order may vary)
            let uuids: Vec<_> = accounts.0.iter().map(|a| a.account_uuid.as_str()).collect();
            assert!(uuids.contains(&account1.account_id.expose_uuid().to_string().as_str()));
            assert!(uuids.contains(&account2.account_id.expose_uuid().to_string().as_str()));

            // Verify names are present
            let names: Vec<_> = accounts
                .0
                .iter()
                .filter_map(|a| a.name.as_deref())
                .collect();
            assert!(names.contains(&"first_account"));
            assert!(names.contains(&"second_account"));
        }

        /// Test z_listaccounts with include_addresses=false omits addresses.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn list_accounts_without_addresses() {
            let network = Network::Consensus(consensus::Network::MainNetwork);
            let wallet = TestWallet::new(network).await.unwrap();

            let _account = wallet.account_builder().build().await.unwrap();

            let handle = wallet.handle().await.unwrap();

            let result = call(handle.as_ref(), Some(false));

            assert!(result.is_ok());
            let accounts = result.unwrap();
            assert_eq!(accounts.0.len(), 1);
            assert!(
                accounts.0[0].addresses.is_none(),
                "Addresses should be omitted when include_addresses is false"
            );
        }
    }
}
