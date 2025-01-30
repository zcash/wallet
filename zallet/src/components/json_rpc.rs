//! JSON-RPC endpoint.
//!
//! This provides JSON-RPC methods that are (mostly) compatible with the `zcashd` wallet
//! RPCs:
//! - Some methods are exactly compatible.
//! - Some methods have the same name but slightly different semantics.
//! - Some methods from the `zcashd` wallet are unsupported.

use zcash_protocol::value::{Zatoshis, COIN};

pub(crate) mod methods;
pub(crate) mod server;

// TODO: https://github.com/zcash/wallet/issues/15
fn value_from_zatoshis(value: Zatoshis) -> f64 {
    (u64::from(value) as f64) / (COIN as f64)
}
