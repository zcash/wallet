use jsonrpsee::proc_macros::rpc;

mod get_notes_count;
mod get_wallet_info;
mod list_accounts;
mod list_unified_receivers;

#[rpc(server)]
pub(crate) trait Rpc {
    #[method(name = "getwalletinfo")]
    fn get_wallet_info(&self) -> get_wallet_info::Response;

    #[method(name = "z_listaccounts")]
    fn list_accounts(&self) -> list_accounts::Response;

    #[method(name = "z_listunifiedreceivers")]
    fn list_unified_receivers(&self, unified_address: &str) -> list_unified_receivers::Response;

    #[method(name = "z_getnotescount")]
    fn get_notes_count(
        &self,
        minconf: Option<u32>,
        as_of_height: Option<i32>,
    ) -> get_notes_count::Response;
}

pub(crate) struct RpcImpl {}

impl RpcImpl {
    /// Creates a new instance of the RPC handler.
    pub(crate) fn new() -> Self {
        Self {}
    }
}

impl RpcServer for RpcImpl {
    fn get_wallet_info(&self) -> get_wallet_info::Response {
        get_wallet_info::call()
    }

    fn list_accounts(&self) -> list_accounts::Response {
        list_accounts::call()
    }

    fn list_unified_receivers(&self, unified_address: &str) -> list_unified_receivers::Response {
        list_unified_receivers::call(unified_address)
    }

    fn get_notes_count(
        &self,
        minconf: Option<u32>,
        as_of_height: Option<i32>,
    ) -> get_notes_count::Response {
        get_notes_count::call(minconf, as_of_height)
    }
}
