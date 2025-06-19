use documented::Documented;
use jsonrpsee::core::RpcResult;
use schemars::JsonSchema;
use serde::Serialize;
use zcash_client_backend::data_api::WalletRead;

use crate::components::{
    database::DbConnection,
    json_rpc::{server::LegacyCode, utils::value_from_zatoshis},
};

/// Response to a `z_gettotalbalance` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = TotalBalance;

/// The total value of funds stored in the wallet.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct TotalBalance {
    /// The total value of unspent transparent outputs, in ZEC
    transparent: f64,

    /// The total value of unspent Sapling and Orchard outputs, in ZEC
    private: f64,

    /// The total value of unspent shielded and transparent outputs, in ZEC
    total: f64,
}

impl TotalBalance {
    fn zero() -> Self {
        TotalBalance {
            transparent: 0.0,
            private: 0.0,
            total: 0.0,
        }
    }
}

pub(super) const PARAM_MINCONF_DESC: &str =
    "Only include notes in transactions confirmed at least this many times.";
pub(super) const PARAM_INCLUDE_WATCH_ONLY_DESC: &str =
    "Also include balance in watchonly addresses.";

pub(crate) fn call(
    wallet: &DbConnection,
    minconf: Option<u32>,
    include_watch_only: Option<bool>,
) -> Response {
    match include_watch_only {
        Some(true) => Ok(()),
        None | Some(false) => Err(LegacyCode::Misc
            .with_message("include_watch_only argument must be set to true (for now)")),
    }?;

    if let Some(summary) = wallet
        .get_wallet_summary(minconf.unwrap_or(1))
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        // TODO: support `include_watch_only = false`
        let mut balance = summary.account_balances().iter().fold(
            TotalBalance::zero(),
            |mut result, (_, balance)| {
                result.transparent += value_from_zatoshis(balance.unshielded_balance().total());
                result.private += value_from_zatoshis(balance.sapling_balance().total());
                result.private += value_from_zatoshis(balance.orchard_balance().total());
                result
            },
        );

        balance.total = balance.transparent + balance.private;

        Ok(balance)
    } else {
        Ok(TotalBalance::zero())
    }
}
