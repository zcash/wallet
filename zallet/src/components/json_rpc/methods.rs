use async_trait::async_trait;
use jsonrpsee::{
    core::{JsonValue, RpcResult},
    proc_macros::rpc,
};
use serde::Serialize;
use tokio::sync::RwLock;
use zaino_state::fetch::FetchServiceSubscriber;

use crate::components::{
    chain_view::ChainView,
    database::{Database, DbHandle},
    keystore::KeyStore,
};

use super::asyncop::{AsyncOperation, ContextInfo, OperationId};

mod get_address_for_account;
mod get_new_account;
mod get_notes_count;
mod get_operation;
mod get_wallet_info;
mod help;
mod list_accounts;
mod list_addresses;
mod list_operation_ids;
mod list_unified_receivers;
mod list_unspent;
mod lock_wallet;
mod openrpc;
mod recover_accounts;
mod unlock_wallet;

#[rpc(server)]
pub(crate) trait Rpc {
    /// List all commands, or get help for a specified command.
    ///
    /// # Arguments
    /// - `command` (string, optional) The command to get help on.
    #[method(name = "help")]
    fn help(&self, command: Option<&str>) -> help::Response;

    /// Returns an OpenRPC schema as a description of this service.
    #[method(name = "rpc.discover")]
    fn openrpc(&self) -> openrpc::Response;

    /// Returns the list of operation ids currently known to the wallet.
    ///
    /// # Arguments
    /// - `status` (string, optional) Filter result by the operation's state e.g. "success".
    #[method(name = "z_listoperationids")]
    async fn list_operation_ids(&self, status: Option<&str>) -> list_operation_ids::Response;

    /// Get operation status and any associated result or error data.
    ///
    /// The operation will remain in memory.
    ///
    /// - If the operation has failed, it will include an error object.
    /// - If the operation has succeeded, it will include the result value.
    /// - If the operation was cancelled, there will be no error object or result value.
    ///
    /// # Arguments
    /// - `operationid` (array, optional) A list of operation ids we are interested in.
    ///   If not provided, examine all operations known to the node.
    #[method(name = "z_getoperationstatus")]
    async fn get_operation_status(&self, operationid: Vec<OperationId>) -> get_operation::Response;

    /// Retrieve the result and status of an operation which has finished, and then remove
    /// the operation from memory.
    ///
    /// - If the operation has failed, it will include an error object.
    /// - If the operation has succeeded, it will include the result value.
    /// - If the operation was cancelled, there will be no error object or result value.
    ///
    /// # Arguments
    /// - `operationid` (array, optional) A list of operation ids we are interested in.
    ///   If not provided, retrieve all finished operations known to the node.
    #[method(name = "z_getoperationresult")]
    async fn get_operation_result(&self, operationid: Vec<OperationId>) -> get_operation::Response;

    /// Returns wallet state information.
    #[method(name = "getwalletinfo")]
    async fn get_wallet_info(&self) -> get_wallet_info::Response;

    /// Stores the wallet decryption key in memory for `timeout` seconds.
    ///
    /// If the wallet is locked, this API must be invoked prior to performing operations
    /// that require the availability of private keys, such as sending funds.
    ///
    /// Issuing the `walletpassphrase` command while the wallet is already unlocked will
    /// set a new unlock time that overrides the old one.
    #[method(name = "walletpassphrase")]
    async fn unlock_wallet(
        &self,
        passphrase: age::secrecy::SecretString,
        timeout: u64,
    ) -> unlock_wallet::Response;

    /// Removes the wallet encryption key from memory, locking the wallet.
    ///
    /// After calling this method, you will need to call `walletpassphrase` again before
    /// being able to call any methods which require the wallet to be unlocked.
    #[method(name = "walletlock")]
    async fn lock_wallet(&self) -> lock_wallet::Response;

    #[method(name = "z_listaccounts")]
    async fn list_accounts(&self) -> list_accounts::Response;

    /// Prepares and returns a new account.
    ///
    /// If the wallet contains more than one UA-compatible HD seed phrase, the `seedfp`
    /// argument must be provided. Available seed fingerprints can be found in the output
    /// of the `listaddresses` RPC method.
    ///
    /// Within a UA-compatible HD seed phrase, accounts are numbered starting from zero;
    /// this RPC method selects the next available sequential account number.
    ///
    /// Each new account is a separate group of funds within the wallet, and adds an
    /// additional performance cost to wallet scanning.
    ///
    /// Use the `z_getaddressforaccount` RPC method to obtain addresses for an account.
    #[method(name = "z_getnewaccount")]
    async fn get_new_account(
        &self,
        account_name: &str,
        seedfp: Option<&str>,
    ) -> get_new_account::Response;

    /// Tells the wallet to track specific accounts.
    ///
    /// Returns the UUIDs within this Zallet instance of the newly-tracked accounts.
    /// Accounts that are already tracked by the wallet are ignored.
    ///
    /// After calling this method, a subsequent call to `z_getnewaccount` will add the
    /// first account with index greater than all indices provided here for the
    /// corresponding `seedfp` (as well as any already tracked by the wallet).
    ///
    /// Each tracked account is a separate group of funds within the wallet, and adds an
    /// additional performance cost to wallet scanning.
    ///
    /// Use the `z_getaddressforaccount` RPC method to obtain addresses for an account.
    ///
    /// # Arguments
    ///
    /// - `accounts` (array, required) An array of JSON objects representing the accounts
    ///   to recover, with the following fields:
    ///   - `name` (string, required)
    ///   - `seedfp` (string, required) The seed fingerprint for the mnemonic phrase from
    ///     which the account is derived. Available seed fingerprints can be found in the
    ///     output of the `listaddresses` RPC method.
    ///   - `zip32_account_index` (numeric, required)
    ///   - `birthday_height` (numeric, required)
    #[method(name = "z_recoveraccounts")]
    async fn recover_accounts(
        &self,
        accounts: Vec<recover_accounts::AccountParameter<'_>>,
    ) -> recover_accounts::Response;

    /// For the given account, derives a Unified Address in accordance with the remaining
    /// arguments:
    ///
    /// - If no list of receiver types is given (or the empty list `[]`), the best and
    ///   second-best shielded receiver types, along with the "p2pkh" (i.e. transparent)
    ///   receiver type, will be used.
    /// - If no diversifier index is given, then:
    ///   - If a transparent receiver would be included (either because no list of
    ///     receiver types is given, or the provided list includes "p2pkh"), the next
    ///     unused index (that is valid for the list of receiver types) will be selected.
    ///   - If only shielded receivers would be included (because a list of receiver types
    ///     is given that does not include "p2pkh"), a time-based index will be selected.
    ///
    /// The account parameter must be a UUID or account number that was previously
    /// generated by a call to the `z_getnewaccount` RPC method. The legacy account number
    /// is only supported for wallets containing a single seed phrase.
    ///
    /// Once a Unified Address has been derived at a specific diversifier index,
    /// re-deriving it (via a subsequent call to `z_getaddressforaccount` with the same
    /// account and index) will produce the same address with the same list of receiver
    /// types. An error will be returned if a different list of receiver types is
    /// requested, including when the empty list `[]` is provided (if the default receiver
    /// types don't match).
    #[method(name = "z_getaddressforaccount")]
    async fn get_address_for_account(
        &self,
        account: JsonValue,
        receiver_types: Option<Vec<String>>,
        diversifier_index: Option<u128>,
    ) -> get_address_for_account::Response;

    /// Lists the addresses managed by this wallet by source.
    ///
    /// Sources include:
    /// - Addresses generated from randomness by a legacy `zcashd` wallet.
    /// - Sapling addresses generated from the legacy `zcashd` HD seed.
    /// - Imported watchonly transparent addresses.
    /// - Shielded addresses tracked using imported viewing keys.
    /// - Addresses derived from mnemonic seed phrases.
    ///
    /// In the case that a source does not have addresses for a value pool, the key
    /// associated with that pool will be absent.
    ///
    /// REMINDER: It is recommended that you back up your wallet files regularly. If you
    /// have not imported externally-produced keys, it only necessary to have backed up
    /// the wallet's key storage file.
    #[method(name = "listaddresses")]
    async fn list_addresses(&self) -> list_addresses::Response;

    #[method(name = "z_listunifiedreceivers")]
    fn list_unified_receivers(&self, unified_address: &str) -> list_unified_receivers::Response;

    /// Returns an array of unspent shielded notes with between minconf and maxconf
    /// (inclusive) confirmations.
    ///
    /// Results may be optionally filtered to only include notes sent to specified
    /// addresses. When `minconf` is 0, unspent notes with zero confirmations are
    /// returned, even though they are not immediately spendable.
    ///
    /// # Arguments
    /// - `minconf` (default = 1)
    #[method(name = "z_listunspent")]
    async fn list_unspent(&self) -> list_unspent::Response;

    #[method(name = "z_getnotescount")]
    async fn get_notes_count(
        &self,
        minconf: Option<u32>,
        as_of_height: Option<i32>,
    ) -> get_notes_count::Response;
}

pub(crate) struct RpcImpl {
    wallet: Database,
    keystore: KeyStore,
    chain_view: ChainView,
    async_ops: RwLock<Vec<AsyncOperation>>,
}

impl RpcImpl {
    /// Creates a new instance of the RPC handler.
    pub(crate) fn new(wallet: Database, keystore: KeyStore, chain_view: ChainView) -> Self {
        Self {
            wallet,
            keystore,
            chain_view,
            async_ops: RwLock::new(Vec::new()),
        }
    }

    async fn wallet(&self) -> RpcResult<DbHandle> {
        self.wallet
            .handle()
            .await
            .map_err(|_| jsonrpsee::types::ErrorCode::InternalError.into())
    }

    async fn chain(&self) -> RpcResult<FetchServiceSubscriber> {
        self.chain_view
            .subscribe()
            .await
            .map(|s| s.inner())
            .map_err(|_| jsonrpsee::types::ErrorCode::InternalError.into())
    }

    async fn start_async<F, T>(&self, (context, f): (Option<ContextInfo>, F)) -> OperationId
    where
        F: Future<Output = RpcResult<T>> + Send + 'static,
        T: Serialize + Send + 'static,
    {
        let mut async_ops = self.async_ops.write().await;
        let op = AsyncOperation::new(context, f).await;
        let op_id = op.operation_id().clone();
        async_ops.push(op);
        op_id
    }
}

#[async_trait]
impl RpcServer for RpcImpl {
    fn help(&self, command: Option<&str>) -> help::Response {
        help::call(command)
    }

    fn openrpc(&self) -> openrpc::Response {
        openrpc::call()
    }

    async fn list_operation_ids(&self, status: Option<&str>) -> list_operation_ids::Response {
        list_operation_ids::call(&self.async_ops.read().await, status).await
    }

    async fn get_operation_status(&self, operationid: Vec<OperationId>) -> get_operation::Response {
        get_operation::status(&self.async_ops.read().await, operationid).await
    }

    async fn get_operation_result(&self, operationid: Vec<OperationId>) -> get_operation::Response {
        get_operation::result(self.async_ops.write().await.as_mut(), operationid).await
    }

    async fn get_wallet_info(&self) -> get_wallet_info::Response {
        get_wallet_info::call(&self.keystore).await
    }

    async fn unlock_wallet(
        &self,
        passphrase: age::secrecy::SecretString,
        timeout: u64,
    ) -> unlock_wallet::Response {
        unlock_wallet::call(&self.keystore, passphrase, timeout).await
    }

    async fn lock_wallet(&self) -> lock_wallet::Response {
        lock_wallet::call(&self.keystore).await
    }

    async fn list_accounts(&self) -> list_accounts::Response {
        list_accounts::call(self.wallet().await?.as_ref())
    }

    async fn get_new_account(
        &self,
        account_name: &str,
        seedfp: Option<&str>,
    ) -> get_new_account::Response {
        get_new_account::call(
            self.wallet().await?.as_mut(),
            &self.keystore,
            self.chain().await?,
            account_name,
            seedfp,
        )
        .await
    }

    async fn recover_accounts(
        &self,
        accounts: Vec<recover_accounts::AccountParameter<'_>>,
    ) -> recover_accounts::Response {
        recover_accounts::call(
            self.wallet().await?.as_mut(),
            &self.keystore,
            self.chain().await?,
            accounts,
        )
        .await
    }

    async fn get_address_for_account(
        &self,
        account: JsonValue,
        receiver_types: Option<Vec<String>>,
        diversifier_index: Option<u128>,
    ) -> get_address_for_account::Response {
        get_address_for_account::call(
            self.wallet().await?.as_mut(),
            account,
            receiver_types,
            diversifier_index,
        )
    }

    async fn list_addresses(&self) -> list_addresses::Response {
        list_addresses::call(self.wallet().await?.as_ref())
    }

    fn list_unified_receivers(&self, unified_address: &str) -> list_unified_receivers::Response {
        list_unified_receivers::call(unified_address)
    }

    async fn list_unspent(&self) -> list_unspent::Response {
        list_unspent::call(self.wallet().await?.as_ref())
    }

    async fn get_notes_count(
        &self,
        minconf: Option<u32>,
        as_of_height: Option<i32>,
    ) -> get_notes_count::Response {
        get_notes_count::call(self.wallet().await?.as_ref(), minconf, as_of_height)
    }
}
