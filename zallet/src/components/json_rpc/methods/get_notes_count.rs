use jsonrpsee::{core::RpcResult, tracing::warn};
use serde::{Deserialize, Serialize};

/// Response to a `z_getnotescount` RPC request.
pub(crate) type Response = RpcResult<GetNotesCount>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct GetNotesCount {
    /// The number of Sprout notes in the wallet.
    ///
    /// Always zero, because Sprout is not supported.
    sprout: u32,

    /// The number of Sapling notes in the wallet.
    sapling: u32,

    /// The number of Orchard notes in the wallet.
    orchard: u32,
}

pub(crate) fn call(minconf: Option<u32>, as_of_height: Option<i32>) -> Response {
    warn!(
        "TODO: Implement z_getnotescount({:?}, {:?})",
        minconf, as_of_height
    );

    Ok(GetNotesCount {
        sprout: 0,
        sapling: 0,
        orchard: 0,
    })
}
