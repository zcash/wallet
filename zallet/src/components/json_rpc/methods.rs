use async_trait::async_trait;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

use crate::components::database::{Database, DbHandle};

mod get_notes_count;
mod get_transaction;
mod get_wallet_info;
mod list_accounts;
mod list_unified_receivers;
mod list_unspent;
mod view_transaction;

#[rpc(server)]
pub(crate) trait Rpc {
    #[method(name = "getwalletinfo")]
    fn get_wallet_info(&self) -> get_wallet_info::Response;

    #[method(name = "z_listaccounts")]
    async fn list_accounts(&self) -> list_accounts::Response;

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
}

impl RpcImpl {
    /// Creates a new instance of the RPC handler.
    pub(crate) fn new(wallet: Database) -> Self {
        Self { wallet }
    }

    async fn wallet(&self) -> RpcResult<DbHandle> {
        self.wallet
            .handle()
            .await
            .map_err(|_| jsonrpsee::types::ErrorCode::InternalError.into())
    }
}

#[async_trait]
impl RpcServer for RpcImpl {
    fn get_wallet_info(&self) -> get_wallet_info::Response {
        get_wallet_info::call()
    }

    async fn list_accounts(&self) -> list_accounts::Response {
        list_accounts::call(self.wallet().await?.as_ref())
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
