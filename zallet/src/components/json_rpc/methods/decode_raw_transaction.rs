use documented::Documented;
use jsonrpsee::core::RpcResult;
use schemars::JsonSchema;
use serde::Serialize;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{TxId, consensus};

use super::get_raw_transaction::{
    Orchard, SaplingOutput, SaplingSpend, TransparentInput, TransparentOutput,
};
use crate::components::json_rpc::{
    server::LegacyCode,
    utils::{JsonZecBalance, value_from_zat_balance},
};

/// Response to a `decoderawtransaction` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// The result type for OpenRPC schema generation.
pub(crate) type ResultType = DecodedTransaction;

/// A decoded transaction.
///
/// Based on zcashd `src/rpc/rawtransaction.cpp:212-338`.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct DecodedTransaction {
    /// The transaction id.
    txid: String,

    /// The transaction's auth digest. For pre-v5 txs this is ffff..ffff.
    authdigest: String,

    /// The transaction size.
    size: u64,

    /// The Overwintered flag.
    overwintered: bool,

    /// The version.
    version: u32,

    /// The version group id (Overwintered txs).
    #[serde(skip_serializing_if = "Option::is_none")]
    versiongroupid: Option<String>,

    /// The lock time.
    locktime: u32,

    /// Last valid block height for mining transaction (Overwintered txs).
    #[serde(skip_serializing_if = "Option::is_none")]
    expiryheight: Option<u32>,

    /// The transparent inputs.
    vin: Vec<TransparentInput>,

    /// The transparent outputs.
    vout: Vec<TransparentOutput>,

    /// The net value of Sapling Spends minus Outputs in ZEC.
    #[serde(rename = "valueBalance")]
    #[serde(skip_serializing_if = "Option::is_none")]
    value_balance: Option<JsonZecBalance>,

    /// The net value of Sapling Spends minus Outputs in zatoshis.
    #[serde(rename = "valueBalanceZat")]
    #[serde(skip_serializing_if = "Option::is_none")]
    value_balance_zat: Option<i64>,

    /// The Sapling spends.
    #[serde(rename = "vShieldedSpend")]
    #[serde(skip_serializing_if = "Option::is_none")]
    v_shielded_spend: Option<Vec<SaplingSpend>>,

    /// The Sapling outputs.
    #[serde(rename = "vShieldedOutput")]
    #[serde(skip_serializing_if = "Option::is_none")]
    v_shielded_output: Option<Vec<SaplingOutput>>,

    /// The Sapling binding sig.
    #[serde(rename = "bindingSig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    binding_sig: Option<String>,

    /// The Orchard bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    orchard: Option<Orchard>,
}

/// Parameter description for OpenRPC schema generation.
pub(super) const PARAM_HEXSTRING_DESC: &str = "The transaction hex string";

/// Decodes a hex-encoded transaction.
pub(crate) fn call(hexstring: &str) -> Response {
    let tx_bytes = hex::decode(hexstring)
        .map_err(|_| LegacyCode::Deserialization.with_static("TX decode failed"))?;

    // The branch ID parameter doesn't affect parsing - tx bytes are self-describing.
    // TODO: Consider proposing Option<BranchId> upstream for decode-only use cases.
    let tx = Transaction::read(tx_bytes.as_slice(), consensus::BranchId::Nu6)
        .map_err(|_| LegacyCode::Deserialization.with_static("TX decode failed"))?;

    let size = tx_bytes.len() as u64;
    let overwintered = tx.version().has_overwinter();

    let (vin, vout) = tx
        .transparent_bundle()
        .map(|bundle| {
            (
                bundle
                    .vin
                    .iter()
                    .map(|tx_in| TransparentInput::encode(tx_in, bundle.is_coinbase()))
                    .collect(),
                bundle
                    .vout
                    .iter()
                    .zip(0..)
                    .map(TransparentOutput::encode)
                    .collect(),
            )
        })
        .unwrap_or_default();

    let (value_balance, value_balance_zat, v_shielded_spend, v_shielded_output, binding_sig) =
        if let Some(bundle) = tx.sapling_bundle() {
            (
                Some(value_from_zat_balance(*bundle.value_balance())),
                Some(bundle.value_balance().into()),
                Some(
                    bundle
                        .shielded_spends()
                        .iter()
                        .map(SaplingSpend::encode)
                        .collect(),
                ),
                Some(
                    bundle
                        .shielded_outputs()
                        .iter()
                        .map(SaplingOutput::encode)
                        .collect(),
                ),
                Some(hex::encode(<[u8; 64]>::from(
                    bundle.authorization().binding_sig,
                ))),
            )
        } else {
            (None, None, None, None, None)
        };

    let orchard = tx
        .version()
        .has_orchard()
        .then(|| Orchard::encode(tx.orchard_bundle()));

    Ok(DecodedTransaction {
        txid: tx.txid().to_string(),
        authdigest: TxId::from_bytes(tx.auth_commitment().as_bytes().try_into().unwrap())
            .to_string(),
        size,
        overwintered,
        version: tx.version().header() & 0x7FFFFFFF,
        versiongroupid: overwintered.then(|| format!("{:08x}", tx.version().version_group_id())),
        locktime: tx.lock_time(),
        expiryheight: overwintered.then(|| tx.expiry_height().into()),
        vin,
        vout,
        value_balance,
        value_balance_zat,
        v_shielded_spend,
        v_shielded_output,
        binding_sig,
        orchard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vectors from zcashd `src/test/rpc_tests.cpp:26-70`

    const V1_TX_HEX: &str = "0100000001a15d57094aa7a21a28cb20b59aab8fc7d1149a3bdbcddba9c622e4f5f6a99ece010000006c493046022100f93bb0e7d8db7bd46e40132d1f8242026e045f03a0efe71bbb8e3f475e970d790221009337cd7f1f929f00cc6ff01f03729b069a7c21b59b1736ddfee5db5946c5da8c0121033b9b137ee87d5a812d6f506efdd37f0affa7ffc310711c06c7f3e097c9447c52ffffffff0100e1f505000000001976a9140389035a9225b3839e2bbf32d826a1e222031fd888ac00000000";

    /// Tests decoding a version 1 transaction.
    #[test]
    fn decode_v1_transaction() {
        let result = call(V1_TX_HEX);
        assert!(result.is_ok());
        let tx = result.unwrap();
        assert_eq!(tx.size, 193);
        assert_eq!(tx.version, 1);
        assert_eq!(tx.locktime, 0);
        assert!(!tx.overwintered);
        assert!(tx.versiongroupid.is_none());
        assert!(tx.expiryheight.is_none());
        assert_eq!(tx.vin.len(), 1);
        assert_eq!(tx.vout.len(), 1);
    }

    /// Tests that "DEADBEEF" (valid hex but invalid transaction) returns an error.
    #[test]
    fn decode_deadbeef_returns_error() {
        let result = call("DEADBEEF");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message(), "TX decode failed");
    }

    /// Tests that "null" (invalid hex) returns an error.
    #[test]
    fn decode_null_returns_error() {
        let result = call("null");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message(), "TX decode failed");
    }

    /// Tests that an empty string returns an error.
    #[test]
    fn decode_empty_returns_error() {
        let result = call("");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.message(), "TX decode failed");
    }

    /// Tests that the error code is RPC_DESERIALIZATION_ERROR (value -22) for decode failures.
    #[test]
    fn decode_error_has_correct_code() {
        let result = call("DEADBEEF");
        let err = result.unwrap_err();
        assert_eq!(err.code(), LegacyCode::Deserialization as i32);
    }
}
