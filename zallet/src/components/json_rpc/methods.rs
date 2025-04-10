use async_trait::async_trait;
use jsonrpsee::{
    core::{JsonValue, RpcResult},
    proc_macros::rpc,
};
use zaino_state::fetch::FetchServiceSubscriber;

use crate::components::{
    chain_view::ChainView,
    database::{Database, DbHandle},
    keystore::KeyStore,
};

mod get_address_for_account;
mod get_notes_count;
mod get_transaction;
mod get_wallet_info;
mod list_accounts;
mod list_addresses;
mod list_unified_receivers;
mod list_unspent;
mod lock_wallet;
mod unlock_wallet;
mod view_transaction;

#[rpc(server)]
pub(crate) trait Rpc {
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

    /// Returns detailed information about in-wallet transaction `txid`.
    ///
    /// This does not include complete information about shielded components of the
    /// transaction; to obtain details about shielded components of the transaction use
    /// `z_viewtransaction`.
    ///
    /// # Parameters
    ///
    /// - `includeWatchonly` (bool, optional, default=false): Whether to include watchonly
    ///   addresses in balance calculation and `details`.
    /// - `verbose`: Must be `false` or omitted.
    /// - `asOfHeight` (numeric, optional, default=-1): Execute the query as if it were
    ///   run when the blockchain was at the height specified by this argument. The
    ///   default is to use the entire blockchain that the node is aware of. -1 can be
    ///   used as in other RPC calls to indicate the current height (including the
    ///   mempool), but this does not support negative values in general. A “future”
    ///   height will fall back to the current height. Any explicit value will cause the
    ///   mempool to be ignored, meaning no unconfirmed tx will be considered.
    ///
    /// # Bitcoin compatibility
    ///
    /// Compatible up to three arguments, but can only use the default value for `verbose`.
    #[method(name = "gettransaction")]
    async fn get_transaction(
        &self,
        txid: &str,
        include_watchonly: Option<bool>,
        verbose: Option<bool>,
        as_of_height: Option<i64>,
    ) -> get_transaction::Response;

    /// Returns detailed shielded information about in-wallet transaction `txid`.
    #[method(name = "z_viewtransaction")]
    async fn view_transaction(&self, txid: &str) -> view_transaction::Response;

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
}

impl RpcImpl {
    /// Creates a new instance of the RPC handler.
    pub(crate) fn new(wallet: Database, keystore: KeyStore, chain_view: ChainView) -> Self {
        Self {
            wallet,
            keystore,
            chain_view,
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
}

#[async_trait]
impl RpcServer for RpcImpl {
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

    async fn get_transaction(
        &self,
        txid: &str,
        include_watchonly: Option<bool>,
        verbose: Option<bool>,
        as_of_height: Option<i64>,
    ) -> get_transaction::Response {
        get_transaction::call(
            self.wallet().await?.as_ref(),
            txid,
            include_watchonly.unwrap_or(false),
            verbose.unwrap_or(false),
            as_of_height.unwrap_or(-1),
        )
    }

    async fn view_transaction(&self, txid: &str) -> view_transaction::Response {
        view_transaction::call(self.wallet().await?.as_ref(), txid)
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
