use jsonrpsee::proc_macros::rpc;

mod get_wallet_info;

#[rpc(server)]
pub(crate) trait Rpc {
    #[method(name = "getwalletinfo")]
    fn get_wallet_info(&self) -> get_wallet_info::Response;
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
}
