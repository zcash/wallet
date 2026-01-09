//! PCZT create method - create an empty PCZT for a new transaction.

use base64ct::{Base64, Encoding};
use documented::Documented;
use jsonrpsee::core::RpcResult;
use pczt::roles::creator::Creator;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zcash_client_backend::data_api::WalletRead;
use zcash_protocol::consensus::{self, NetworkType, Parameters};

use crate::components::{
    database::DbConnection,
    json_rpc::server::LegacyCode,
};

/// Response to a `pczt_create` RPC request.
pub(crate) type Response = RpcResult<ResultType>;
pub(crate) type ResultType = CreateResult;

/// Parameters for the `pczt_create` RPC method.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub(crate) struct CreateParams {
    /// The expiry height for the transaction. If not specified, defaults to
    /// the current chain height plus 40 blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry_height: Option<u32>,

    /// The lock time for the transaction. Defaults to 0 (no lock time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_time: Option<u32>,
}

/// Result of creating a new PCZT.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct CreateResult {
    /// The base64-encoded empty PCZT.
    pub pczt: String,

    /// The expiry height set on the PCZT.
    pub expiry_height: u32,

    /// The consensus branch ID.
    pub consensus_branch_id: String,
}

pub(super) const PARAM_EXPIRY_HEIGHT_DESC: &str =
    "The expiry height for the transaction. Defaults to current height + 40.";
pub(super) const PARAM_LOCK_TIME_DESC: &str =
    "The lock time for the transaction. Defaults to 0.";

/// Creates an empty PCZT that can be used to build a transaction.
///
/// The PCZT will be initialized with the current consensus parameters and
/// can be funded using `pczt_fund`.
pub(crate) fn call(
    wallet: &DbConnection,
    expiry_height: Option<u32>,
    lock_time: Option<u32>,
) -> Response {
    // Get the current chain height
    let chain_height = wallet
        .chain_height()
        .map_err(|e| LegacyCode::Database.with_message(format!("Failed to get chain height: {e}")))?
        .ok_or_else(|| LegacyCode::InWarmup.with_static("Wallet sync required"))?;

    // Calculate expiry height (default: current height + 40 blocks)
    let expiry_height = expiry_height.unwrap_or(
        u32::from(chain_height)
            .checked_add(40)
            .ok_or_else(|| LegacyCode::Misc.with_static("Chain height overflow"))?,
    );

    // Get the consensus branch ID for the target height
    let params = wallet.params();
    let target_height = consensus::BlockHeight::from_u32(expiry_height);
    let branch_id = consensus::BranchId::for_height(params, target_height);

    // Get coin type based on network
    let coin_type = match params.network_type() {
        NetworkType::Main => 133,    // Zcash mainnet
        NetworkType::Test | NetworkType::Regtest => 1, // Testnet
    };

    // Get the lock time (default: 0)
    let lock_time = lock_time.unwrap_or(0);

    // Create the PCZT using the Creator role
    // We use empty anchors since the PCZT will be funded later with actual notes
    let creator = Creator::new(
        u32::from(branch_id),
        expiry_height,
        coin_type,
        [0u8; 32], // Sapling anchor placeholder (will be set during funding)
        [0u8; 32], // Orchard anchor placeholder (will be set during funding)
    );

    let creator = if lock_time > 0 {
        creator.with_fallback_lock_time(lock_time)
    } else {
        creator
    };

    let pczt = creator.build();

    // Serialize and encode
    let pczt_bytes = pczt.serialize();
    let pczt_base64 = Base64::encode_string(&pczt_bytes);

    Ok(CreateResult {
        pczt: pczt_base64,
        expiry_height,
        consensus_branch_id: format!("{:08x}", u32::from(branch_id)),
    })
}
