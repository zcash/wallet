use documented::Documented;
use jsonrpsee::core::RpcResult;
use ripemd::Ripemd160;
use schemars::JsonSchema;
use secp256k1::PublicKey;
use serde::Serialize;
use sha2::{Digest, Sha256};
use transparent::address::TransparentAddress;
use zcash_keys::encoding::AddressCodec;
use zcash_script::{
    script::{Asm, Code},
    solver::{self, ScriptKind},
};

use crate::{components::json_rpc::server::LegacyCode, network::Network};

pub(crate) type Response = RpcResult<ResultType>;

/// The result of decoding a script.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
pub(crate) struct ResultType {
    /// String representation of the script public key.
    asm: String,
    /// The type of script.
    ///
    /// One of: `pubkeyhash`, `scripthash`, `pubkey`, `multisig`, `nulldata`, `nonstandard`.
    #[serde(rename = "type")]
    kind: &'static str,
    /// The required number of signatures.
    ///
    /// Omitted for scripts that don't contain identifiable addresses (such as
    /// non-standard or null-data scripts).
    #[serde(rename = "reqSigs", skip_serializing_if = "Option::is_none")]
    req_sigs: Option<u8>,
    /// The addresses associated with this script.
    ///
    /// Omitted for scripts that don't contain identifiable addresses (such as
    /// non-standard or null-data scripts).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    addresses: Vec<String>,
    /// The P2SH address for this script.
    p2sh: String,
}

pub(super) const PARAM_HEXSTRING_DESC: &str = "The hex-encoded script.";

/// Decodes a hex-encoded script.
///
/// # Arguments
/// - `params`: Network parameters for address encoding.
/// - `hexstring`: The hex-encoded script.
pub(crate) fn call(params: &Network, hexstring: &str) -> Response {
    let script_bytes = hex::decode(hexstring)
        .map_err(|_| LegacyCode::Deserialization.with_static("Hex decoding failed"))?;

    let script_code = Code(script_bytes);
    let asm = to_zcashd_asm(&script_code.to_asm(false));

    let (kind, req_sigs, addresses) = detect_script_info(&script_code, params);
    let p2sh = calculate_p2sh_address(&script_code.0, params);

    Ok(ResultType {
        asm,
        kind,
        req_sigs,
        addresses,
        p2sh,
    })
}

/// Converts zcash_script ASM output to zcashd-compatible format.
///
/// The zcash_script crate outputs "OP_1" through "OP_16" and "OP_1NEGATE",
/// but zcashd outputs "1" through "16" and "-1" respectively.
///
/// Reference: https://github.com/zcash/zcash/blob/v6.11.0/src/script/script.cpp#L19-L40
///
/// TODO: Remove this function once zcash_script is upgraded past 0.4.x,
///       as `to_asm()` will natively output zcashd-compatible format.
///       See https://github.com/ZcashFoundation/zcash_script/pull/289
pub(super) fn to_zcashd_asm(asm: &str) -> String {
    asm.split(' ')
        .map(|token| match token {
            "OP_1NEGATE" => "-1",
            "OP_1" => "1",
            "OP_2" => "2",
            "OP_3" => "3",
            "OP_4" => "4",
            "OP_5" => "5",
            "OP_6" => "6",
            "OP_7" => "7",
            "OP_8" => "8",
            "OP_9" => "9",
            "OP_10" => "10",
            "OP_11" => "11",
            "OP_12" => "12",
            "OP_13" => "13",
            "OP_14" => "14",
            "OP_15" => "15",
            "OP_16" => "16",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Computes the Hash160 of the given data.
fn hash160(data: &[u8]) -> [u8; 20] {
    let sha_hash = Sha256::digest(data);
    Ripemd160::digest(sha_hash).into()
}

/// Converts a raw public key to its P2PKH address.
fn pubkey_to_p2pkh_address(pubkey_bytes: &[u8], params: &Network) -> Option<String> {
    let pubkey = PublicKey::from_slice(pubkey_bytes).ok()?;
    let addr = TransparentAddress::from_pubkey(&pubkey);
    Some(addr.encode(params))
}

/// Calculates the P2SH address for a given script.
fn calculate_p2sh_address(script_bytes: &[u8], params: &Network) -> String {
    let hash = hash160(script_bytes);
    TransparentAddress::ScriptHash(hash).encode(params)
}

/// Detects the script type and extracts associated information.
///
/// Returns a tuple of (type_name, required_sigs, addresses).
fn detect_script_info(
    script_code: &Code,
    params: &Network,
) -> (&'static str, Option<u8>, Vec<String>) {
    script_code
        .to_component()
        .ok()
        .and_then(|c| c.refine().ok())
        .and_then(|component| solver::standard(&component))
        .map(|script_kind| match script_kind {
            ScriptKind::PubKeyHash { hash } => {
                let addr = TransparentAddress::PublicKeyHash(hash);
                ("pubkeyhash", Some(1), vec![addr.encode(params)])
            }
            ScriptKind::ScriptHash { hash } => {
                let addr = TransparentAddress::ScriptHash(hash);
                ("scripthash", Some(1), vec![addr.encode(params)])
            }
            ScriptKind::PubKey { data } => {
                let addresses: Vec<String> = pubkey_to_p2pkh_address(data.as_slice(), params)
                    .into_iter()
                    .collect();
                let req_sigs = if addresses.is_empty() { None } else { Some(1) };
                ("pubkey", req_sigs, addresses)
            }
            ScriptKind::MultiSig { required, pubkeys } => {
                let addresses: Vec<String> = pubkeys
                    .iter()
                    .filter_map(|pk| pubkey_to_p2pkh_address(pk.as_slice(), params))
                    .collect();
                let req_sigs = if addresses.is_empty() {
                    None
                } else {
                    Some(required)
                };
                ("multisig", req_sigs, addresses)
            }
            ScriptKind::NullData { .. } => ("nulldata", None, vec![]),
        })
        .unwrap_or(("nonstandard", None, vec![]))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use zcash_protocol::consensus;

    // From zcashd qa/rpc-tests/decodescript.py:17-22
    const ZCASHD_PUBLIC_KEY: &str =
        "03b0da749730dc9b4b1f4a14d6902877a92541f5368778853d9c4a0cb7802dcfb2";
    const ZCASHD_PUBLIC_KEY_HASH: &str = "11695b6cd891484c2d49ec5aa738ec2b2f897777";

    fn mainnet() -> Network {
        Network::Consensus(consensus::Network::MainNetwork)
    }

    fn testnet() -> Network {
        Network::Consensus(consensus::Network::TestNetwork)
    }

    #[test]
    fn decode_p2pkh_script() {
        // From zcashd qa/rpc-tests/decodescript.py:65
        // P2PKH: OP_DUP OP_HASH160 <pubkey_hash> OP_EQUALVERIFY OP_CHECKSIG
        let script_hex = format!("76a914{ZCASHD_PUBLIC_KEY_HASH}88ac");
        let result = call(&mainnet(), &script_hex).unwrap();

        assert_eq!(result.kind, "pubkeyhash");
        assert_eq!(result.req_sigs, Some(1));
        assert_eq!(result.addresses.len(), 1);
        assert!(result.addresses[0].starts_with("t1"));
        assert_eq!(
            result.asm,
            format!("OP_DUP OP_HASH160 {ZCASHD_PUBLIC_KEY_HASH} OP_EQUALVERIFY OP_CHECKSIG")
        );
    }

    #[test]
    fn decode_p2sh_script() {
        // From zcashd qa/rpc-tests/decodescript.py:73
        // P2SH: OP_HASH160 <script_hash> OP_EQUAL
        let script_hex = format!("a914{ZCASHD_PUBLIC_KEY_HASH}87");
        let result = call(&mainnet(), &script_hex).unwrap();

        assert_eq!(result.kind, "scripthash");
        assert_eq!(result.req_sigs, Some(1));
        assert_eq!(result.addresses.len(), 1);
        assert!(result.addresses[0].starts_with("t3"));
        assert_eq!(
            result.asm,
            format!("OP_HASH160 {ZCASHD_PUBLIC_KEY_HASH} OP_EQUAL")
        );
    }

    #[test]
    fn decode_p2pk_script() {
        // From zcashd qa/rpc-tests/decodescript.py:57
        // P2PK: <pubkey> OP_CHECKSIG
        // 0x21 = 33 bytes push opcode
        let script_hex = format!("21{ZCASHD_PUBLIC_KEY}ac");
        let result = call(&mainnet(), &script_hex).unwrap();

        assert_eq!(result.kind, "pubkey");
        assert_eq!(result.req_sigs, Some(1));
        assert_eq!(result.addresses.len(), 1);
        assert!(result.addresses[0].starts_with("t1"));
        assert_eq!(result.asm, format!("{ZCASHD_PUBLIC_KEY} OP_CHECKSIG"));
    }

    #[test]
    fn decode_multisig_script() {
        // From zcashd qa/rpc-tests/decodescript.py:69
        // 2-of-3 Multisig: OP_2 <pubkey> <pubkey> <pubkey> OP_3 OP_CHECKMULTISIG
        // Uses the same pubkey repeated 3 times (valid for testing)
        let script_hex = format!(
            "52\
             21{pk}\
             21{pk}\
             21{pk}\
             53ae",
            pk = ZCASHD_PUBLIC_KEY
        );
        let result = call(&mainnet(), &script_hex).unwrap();

        assert_eq!(result.kind, "multisig");
        assert_eq!(result.req_sigs, Some(2));
        assert_eq!(result.addresses.len(), 3);
        // All addresses should be the same since we used the same pubkey
        assert_eq!(result.addresses.iter().collect::<HashSet<_>>().len(), 1);
        // Verify ASM uses decimal numbers for OP_2 and OP_3
        assert!(result.asm.starts_with("2 "));
        assert!(result.asm.contains(" 3 OP_CHECKMULTISIG"));
    }

    #[test]
    fn decode_nulldata_script() {
        // From zcashd qa/rpc-tests/decodescript.py:77
        // OP_RETURN with signature-like data (crafted to resemble a DER signature)
        let script_hex = "6a48304502207fa7a6d1e0ee81132a269ad84e68d695483745cde8b541e\
            3bf630749894e342a022100c1f7ab20e13e22fb95281a870f3dcf38d782e53023ee31\
            3d741ad0cfbc0c509001";
        let result = call(&mainnet(), script_hex).unwrap();

        assert_eq!(result.kind, "nulldata");
        assert_eq!(result.req_sigs, None);
        assert!(result.addresses.is_empty());
        assert!(result.asm.starts_with("OP_RETURN"));
    }

    #[test]
    fn decode_nonstandard_script() {
        // OP_TRUE (0x51)
        let script_hex = "51";
        let result = call(&mainnet(), script_hex).unwrap();

        assert_eq!(result.kind, "nonstandard");
        assert_eq!(result.req_sigs, None);
        assert!(result.addresses.is_empty());
        // ASM should show "1" for OP_1/OP_TRUE
        assert_eq!(result.asm, "1");
    }

    #[test]
    fn decode_empty_script() {
        let result = call(&mainnet(), "").unwrap();

        assert_eq!(result.kind, "nonstandard");
        assert_eq!(result.req_sigs, None);
        assert!(result.addresses.is_empty());
        assert!(result.asm.is_empty());
        // P2SH should still be computed (hash of empty script)
        assert!(result.p2sh.starts_with("t3"));
    }

    #[test]
    fn decode_invalid_hex() {
        let result = call(&mainnet(), "not_hex");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code(), LegacyCode::Deserialization as i32);
        assert_eq!(err.message(), "Hex decoding failed");
    }

    #[test]
    fn decode_p2pkh_testnet() {
        // Same P2PKH script from zcashd test vectors but on testnet
        let script_hex = format!("76a914{ZCASHD_PUBLIC_KEY_HASH}88ac");
        let result = call(&testnet(), &script_hex).unwrap();

        assert_eq!(result.kind, "pubkeyhash");
        // Testnet addresses start with "tm" for P2PKH
        assert!(result.addresses[0].starts_with("tm"));
        // P2SH testnet addresses start with "t2"
        assert!(result.p2sh.starts_with("t2"));
    }

    #[test]
    fn p2sh_address_calculation() {
        // Verify P2SH address is correctly calculated for a P2PKH script
        let script_hex = format!("76a914{ZCASHD_PUBLIC_KEY_HASH}88ac");
        let result = call(&mainnet(), &script_hex).unwrap();

        // P2SH address should be computed for any script type
        let script_bytes = hex::decode(&script_hex).unwrap();
        let expected_p2sh = calculate_p2sh_address(&script_bytes, &mainnet());
        assert_eq!(result.p2sh, expected_p2sh);
    }
}
