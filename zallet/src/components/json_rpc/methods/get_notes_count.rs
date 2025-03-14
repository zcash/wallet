use jsonrpsee::core::RpcResult;
use serde::{Deserialize, Serialize};
use zcash_client_backend::data_api::{InputSource, NoteFilter, WalletRead};
use zcash_protocol::{value::Zatoshis, ShieldedProtocol};

use crate::components::{json_rpc::server::LegacyCode, wallet::WalletConnection};

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

pub(crate) fn call(
    wallet: &WalletConnection,
    minconf: Option<u32>,
    as_of_height: Option<i32>,
) -> Response {
    // TODO: Switch to an approach that can respect `minconf` and `as_of_height`.
    if minconf.is_some() || as_of_height.is_some() {
        return Err(LegacyCode::InvalidParameter
            .with_static("minconf and as_of_height parameters are not yet supported"));
    }

    let selector = NoteFilter::ExceedsMinValue(Zatoshis::ZERO);

    let mut sapling = 0;
    let mut orchard = 0;
    for account_id in wallet
        .get_account_ids()
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
    {
        let account_metadata = wallet
            .get_account_metadata(account_id, &selector, &[])
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?;

        if let Some(note_count) = account_metadata.note_count(ShieldedProtocol::Sapling) {
            sapling += note_count as u32;
        }
        if let Some(note_count) = account_metadata.note_count(ShieldedProtocol::Orchard) {
            orchard += note_count as u32;
        }
    }

    Ok(GetNotesCount {
        sprout: 0,
        sapling,
        orchard,
    })
}
