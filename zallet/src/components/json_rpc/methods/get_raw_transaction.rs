#![allow(deprecated)] // For zaino

use documented::Documented;
use jsonrpsee::core::RpcResult;
use sapling::bundle::{OutputDescription, SpendDescription};
use schemars::JsonSchema;
use serde::Serialize;
use transparent::bundle::{TxIn, TxOut};
use zaino_state::{FetchServiceError, FetchServiceSubscriber, LightWalletIndexer, ZcashIndexer};
use zcash_protocol::{
    TxId,
    consensus::{self, BlockHeight},
    value::ZatBalance,
};

use crate::components::{
    database::DbConnection,
    json_rpc::{
        server::LegacyCode,
        utils::{JsonZec, JsonZecBalance, parse_txid, value_from_zat_balance, value_from_zatoshis},
    },
};

/// Response to a `getrawtransaction` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// The `getrawtransaction` response.
#[derive(Clone, Debug, Serialize, Documented, JsonSchema)]
#[serde(untagged)]
pub(crate) enum ResultType {
    /// The serialized, hex-encoded data for `txid`.
    Concise(String),

    /// An object describing the transaction in detail.
    Verbose(Box<Transaction>),
}

/// Verbose information about a transaction.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct Transaction {
    /// Whether specified block is in the active chain or not.
    ///
    /// Only present with explicit `blockhash` argument.
    #[serde(skip_serializing_if = "Option::is_none")]
    in_active_chain: Option<bool>,

    /// The serialized, hex-encoded data for the transaction identified by `txid`.
    hex: String,

    /// The transaction ID (same as provided).
    txid: String,

    /// The transaction's authorizing data commitment.
    ///
    /// For pre-v5 transactions this will be `ffff..ffff`.
    ///
    /// Encoded as a byte-reversed hex string to match `txid`.
    authdigest: String,

    /// The network-serialized transaction size.
    size: u64,

    /// Whether the `overwintered` flag is set.
    overwintered: bool,

    /// The transaction version.
    version: u32,

    /// The version group ID.
    ///
    /// Omitted if `overwintered` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    versiongroupid: Option<String>,

    /// The lock time.
    locktime: u32,

    /// The block height after which the transaction expires.
    ///
    /// Omitted if `overwintered` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    expiryheight: Option<u32>,

    /// The transparent inputs to the transaction.
    vin: Vec<TransparentInput>,

    /// The transparent outputs from the transaction.
    vout: Vec<TransparentOutput>,

    /// The Sprout spends and outputs of the transaction.
    #[cfg(zallet_unimplemented)]
    vjoinsplit: Vec<JoinSplit>,

    /// The net value of Sapling Spends minus Outputs in ZEC.
    ///
    /// Omitted if `version < 4`.
    #[serde(rename = "valueBalance")]
    #[serde(skip_serializing_if = "Option::is_none")]
    value_balance: Option<JsonZecBalance>,

    /// The net value of Sapling Spends minus Outputs in zatoshis.
    ///
    /// Omitted if `version < 4`.
    #[serde(rename = "valueBalanceZat")]
    #[serde(skip_serializing_if = "Option::is_none")]
    value_balance_zat: Option<i64>,

    /// Omitted if `version < 4`.
    #[serde(rename = "vShieldedSpend")]
    #[serde(skip_serializing_if = "Option::is_none")]
    v_shielded_spend: Option<Vec<SaplingSpend>>,

    /// Omitted if `version < 4`.
    #[serde(rename = "vShieldedOutput")]
    #[serde(skip_serializing_if = "Option::is_none")]
    v_shielded_output: Option<Vec<SaplingOutput>>,

    /// The Sapling binding signature, encoded as a hex string.
    ///
    /// Omitted if `version < 4`, or both `vShieldedSpend` and `vShieldedOutput` are empty.
    #[serde(rename = "bindingSig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    binding_sig: Option<String>,

    /// JSON object with Orchard-specific information.
    ///
    /// Omitted if `version < 5`.
    #[serde(skip_serializing_if = "Option::is_none")]
    orchard: Option<Orchard>,

    /// The `joinSplitSig` public validating key.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    ///
    /// Omitted if `version < 2` or `vjoinsplit` is empty.
    #[cfg(zallet_unimplemented)]
    #[serde(rename = "joinSplitPubKey")]
    #[serde(skip_serializing_if = "Option::is_none")]
    join_split_pub_key: Option<String>,

    /// The Sprout binding signature, encoded as a hex string.
    ///
    /// Omitted if `version < 2` or `vjoinsplit` is empty.
    #[cfg(zallet_unimplemented)]
    #[serde(rename = "joinSplitSig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    join_split_sig: Option<String>,

    /// The hash of the block that the transaction is mined in, if any.
    ///
    /// Omitted if the transaction is not known to be mined in any block.
    #[serde(skip_serializing_if = "Option::is_none")]
    blockhash: Option<String>,

    /// The height of the block that the transaction is mined in, or -1 if that block is
    /// not in the current best chain.
    ///
    /// Omitted if `blockhash` is either omitted or unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<i32>,

    /// The number of confirmations the transaction has, or 0 if the block it is mined in
    /// is not in the current best chain.
    ///
    /// Omitted if `blockhash` is either omitted or unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmations: Option<u32>,

    /// The transaction time in seconds since epoch (Jan 1 1970 GMT).
    ///
    /// This is always identical to `blocktime`.
    ///
    /// Omitted if `blockhash` is either omitted or not in the current best chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    time: Option<i64>,

    /// The block time in seconds since epoch (Jan 1 1970 GMT) for the block that the
    /// transaction is mined in.
    ///
    /// Omitted if `blockhash` is either omitted or not in the current best chain.
    #[serde(skip_serializing_if = "Option::is_none")]
    blocktime: Option<i64>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TransparentInput {
    /// The coinbase `scriptSig`, encoded as a hex string.
    ///
    /// Omitted if this is not a coinbase transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    coinbase: Option<String>,

    /// The transaction ID of the output being spent.
    ///
    /// Omitted if this is a coinbase transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    txid: Option<String>,

    /// The index of the output being spent within the transaction identified by `txid`.
    ///
    /// Omitted if this is a coinbase transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    vout: Option<u32>,

    /// Omitted if this is a coinbase transaction.
    #[serde(rename = "scriptSig")]
    #[serde(skip_serializing_if = "Option::is_none")]
    script_sig: Option<TransparentScriptSig>,

    /// The script sequence number.
    sequence: u32,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TransparentScriptSig {
    /// The assembly string representation of the script.
    asm: String,

    /// The serialized script, encoded as a hex string.
    hex: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TransparentOutput {
    /// The value in ZEC.
    value: JsonZec,

    /// The value in zatoshis.
    #[serde(rename = "valueZat")]
    value_zat: u64,

    /// The value in zatoshis.
    #[serde(rename = "valueSat")]
    value_sat: u64,

    /// The index of the output within the transaction's `vout` field.
    n: u16,

    #[serde(rename = "scriptPubKey")]
    script_pub_key: TransparentScriptPubKey,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct TransparentScriptPubKey {
    /// The assembly string representation of the script.
    asm: String,

    /// The serialized script, encoded as a hex string.
    hex: String,

    /// The required number of signatures to spend this output.
    #[serde(rename = "reqSigs")]
    req_sigs: u8,

    /// The type of script.
    ///
    /// One of `["pubkey", "pubkeyhash", "scripthash", "multisig", "nulldata", "nonstandard"]`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// Array of the transparent P2PKH addresses involved in the script.
    addresses: Vec<String>,
}

#[cfg(zallet_unimplemented)]
#[derive(Clone, Debug, Serialize, JsonSchema)]
struct JoinSplit {
    /// The public input value in ZEC.
    vpub_old: JsonZec,

    /// The public input value in zatoshis.
    #[serde(rename = "vpub_oldZat")]
    vpub_old_zat: u64,

    /// The public output value in ZEC.
    vpub_new: JsonZec,

    /// The public output value in zatoshis.
    #[serde(rename = "vpub_newZat")]
    vpub_new_zat: u64,

    /// The anchor.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    anchor: String,

    /// Array of input note nullifiers.
    ///
    /// Encoded as byte-reversed hex strings for legacy reasons.
    nullifiers: Vec<String>,

    /// Array of output note commitments.
    ///
    /// Encoded as byte-reversed hex strings for legacy reasons.
    commitments: Vec<String>,

    /// The onetime public key used to encrypt the ciphertexts.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    #[serde(rename = "onetimePubKey")]
    onetime_pub_key: String,

    /// The random seed.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    #[serde(rename = "randomSeed")]
    random_seed: String,

    /// Array of input note MACs.
    ///
    /// Encoded as byte-reversed hex strings for legacy reasons.
    macs: Vec<String>,

    /// The zero-knowledge proof, encoded as a hex string.
    proof: String,

    /// Array of output note ciphertexts, encoded as hex strings.
    ciphertexts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct SaplingSpend {
    /// Value commitment to the input note.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    cv: String,

    /// Merkle root of the Sapling note commitment tree.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    anchor: String,

    /// The nullifier of the input note.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    nullifier: String,

    /// The randomized public key for `spendAuthSig`.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    rk: String,

    /// A zero-knowledge proof using the Sapling Spend circuit, encoded as a hex string.
    proof: String,

    /// A signature authorizing this Spend, encoded as a hex string.
    #[serde(rename = "spendAuthSig")]
    spend_auth_sig: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct SaplingOutput {
    /// Value commitment to the output note.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    cv: String,

    /// The u-coordinate of the note commitment for the output note.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    cmu: String,

    /// A Jubjub public key.
    ///
    /// Encoded as a byte-reversed hex string for legacy reasons.
    #[serde(rename = "ephemeralKey")]
    ephemeral_key: String,

    /// The output note encrypted to the recipient, encoded as a hex string.
    #[serde(rename = "encCiphertext")]
    enc_ciphertext: String,

    /// A ciphertext enabling the sender to recover the output note, encoded as a hex
    /// string.
    #[serde(rename = "outCiphertext")]
    out_ciphertext: String,

    /// A zero-knowledge proof using the Sapling Output circuit, encoded as a hex string.
    proof: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct Orchard {
    /// The Orchard Actions for the transaction.
    actions: Vec<OrchardAction>,

    /// The net value of Orchard Actions in ZEC.
    #[serde(rename = "valueBalance")]
    value_balance: JsonZecBalance,

    /// The net value of Orchard Actions in zatoshis.
    #[serde(rename = "valueBalanceZat")]
    value_balance_zat: i64,

    /// The Orchard bundle flags.
    ///
    /// Omitted if `actions` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    flags: Option<OrchardFlags>,

    /// A root of the Orchard note commitment tree at some block height in the past,
    /// encoded as a hex string.
    ///
    /// Omitted if `actions` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<String>,

    /// Encoding of aggregated zk-SNARK proofs for Orchard Actions, encoded as a hex
    /// string.
    ///
    /// Omitted if `actions` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<String>,

    /// An Orchard binding signature on the SIGHASH transaction hash, encoded as a hex
    /// string.
    ///
    /// Omitted if `actions` is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "bindingSig")]
    binding_sig: Option<String>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct OrchardAction {
    /// A value commitment to the net value of the input note minus the output note,
    /// encoded as a hex string.
    cv: String,

    /// The nullifier of the input note, encoded as a hex string.
    nullifier: String,

    /// The randomized validating key for `spendAuthSig`, encoded as a hex string.
    rk: String,

    /// The x-coordinate of the note commitment for the output note, encoded as a hex
    /// string.
    cmx: String,

    /// An encoding of an ephemeral Pallas public key, encoded as a hex string.
    #[serde(rename = "ephemeralKey")]
    ephemeral_key: String,

    /// The output note encrypted to the recipient, encoded as a hex string.
    #[serde(rename = "encCiphertext")]
    enc_ciphertext: String,

    /// A ciphertext enabling the sender to recover the output note, encoded as a hex
    /// string.
    #[serde(rename = "outCiphertext")]
    out_ciphertext: String,

    /// A signature authorizing the spend in this Action, encoded as a hex string.
    #[serde(rename = "spendAuthSig")]
    spend_auth_sig: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
struct OrchardFlags {
    /// Whether spends are enabled in this Orchard bundle.
    #[serde(rename = "enableSpends")]
    enable_spends: bool,

    /// Whether outputs are enabled in this Orchard bundle.
    #[serde(rename = "enableOutputs")]
    enable_outputs: bool,
}

pub(super) const PARAM_TXID_DESC: &str = "The ID of the transaction to fetch.";
pub(super) const PARAM_VERBOSE_DESC: &str = "If 0, return a string of hex-encoded data. If non-zero, return a JSON object with information about `txid`";
pub(super) const PARAM_BLOCKHASH_DESC: &str = "The block in which to look for the transaction.";

pub(crate) async fn call(
    wallet: &DbConnection,
    chain: FetchServiceSubscriber,
    txid_str: &str,
    verbose: Option<u64>,
    blockhash: Option<String>,
) -> Response {
    let _txid = parse_txid(txid_str)?;
    let verbose = verbose.is_some_and(|v| v != 0);

    // TODO: We can't support this via the current Zaino API; wait for `ChainIndex`.
    //       https://github.com/zcash/wallet/issues/237
    if blockhash.is_some() {
        return Err(
            LegacyCode::InvalidParameter.with_static("blockhash argument must be unset (for now).")
        );
    }

    let tx = match chain.get_raw_transaction(txid_str.into(), Some(1)).await {
        // TODO: Zaino should have a Rust API for fetching tx details, instead of
        //       requiring us to specify a verbosity and then deal with an enum variant
        //       that should never occur.
        //       https://github.com/zcash/wallet/issues/237
        Ok(zebra_rpc::methods::GetRawTransaction::Raw(_)) => unreachable!(),
        Ok(zebra_rpc::methods::GetRawTransaction::Object(tx)) => Ok(tx),
        // TODO: Zaino is not correctly parsing the error response, so we
        // can't look for `LegacyCode::InvalidAddressOrKey`. Instead match
        // on these three possible error messages:
        // - "No such mempool or blockchain transaction" (zcashd -txindex)
        // - "No such mempool transaction." (zcashd)
        // - "No such mempool or main chain transaction" (zebrad)
        Err(FetchServiceError::RpcError(e)) if e.message.contains("No such mempool") => {
            Err(LegacyCode::InvalidAddressOrKey
                .with_static("No such mempool or blockchain transaction"))
        }
        Err(e) => Err(LegacyCode::Database.with_message(e.to_string())),
    }?;

    // TODO: Once we migrate to `ChainIndex`, fetch these via the snapshot.
    //       https://github.com/zcash/wallet/issues/237
    let blockhash = tx.block_hash().map(|hash| hash.to_string());
    let height = tx.height();
    let confirmations = tx.confirmations();
    let time = tx.time();
    let blocktime = tx.block_time();

    let tx_hex = hex::encode(tx.hex());
    if !verbose {
        return Ok(ResultType::Concise(tx_hex));
    }

    let mempool_height = BlockHeight::from_u32(
        chain
            .get_latest_block()
            .await
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            .height
            .try_into()
            .expect("not our problem"),
    ) + 1;

    let consensus_branch_id = consensus::BranchId::for_height(
        wallet.params(),
        tx.height()
            .and_then(|h| u32::try_from(h).ok().map(BlockHeight::from_u32))
            .unwrap_or(mempool_height),
    );
    let tx =
        zcash_primitives::transaction::Transaction::read(tx.hex().as_ref(), consensus_branch_id)
            .expect("guaranteed to be parseable by Zaino");

    let size = (tx_hex.len() / 2) as u64;

    let overwintered = tx.version().has_overwinter();

    let (vin, vout) = match tx.transparent_bundle() {
        Some(bundle) => (
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
        ),
        _ => (vec![], vec![]),
    };

    #[cfg(zallet_unimplemented)]
    let (vjoinsplit, join_split_pub_key, join_split_sig) = match tx.sprout_bundle() {
        Some(bundle) if !bundle.joinsplits.is_empty() => (
            bundle.joinsplits.iter().map(JoinSplit::encode).collect(),
            Some(TxId::from_bytes(bundle.joinsplit_pubkey).to_string()),
            Some(hex::encode(bundle.joinsplit_sig)),
        ),
        _ => (vec![], None, None),
    };

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

    Ok(ResultType::Verbose(Box::new(Transaction {
        in_active_chain: None,
        hex: tx_hex,
        txid: txid_str.to_ascii_lowercase(),
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
        #[cfg(zallet_unimplemented)]
        vjoinsplit,
        value_balance,
        value_balance_zat,
        v_shielded_spend,
        v_shielded_output,
        binding_sig,
        orchard,
        #[cfg(zallet_unimplemented)]
        join_split_pub_key,
        #[cfg(zallet_unimplemented)]
        join_split_sig,
        blockhash,
        height,
        confirmations,
        time,
        blocktime,
    })))
}

impl TransparentInput {
    fn encode(tx_in: &TxIn<transparent::bundle::Authorized>, is_coinbase: bool) -> Self {
        let script_hex = hex::encode(&tx_in.script_sig().0.0);

        if is_coinbase {
            Self {
                coinbase: Some(script_hex),
                txid: None,
                vout: None,
                script_sig: None,
                sequence: tx_in.sequence(),
            }
        } else {
            Self {
                coinbase: None,
                txid: Some(TxId::from_bytes(*tx_in.prevout().hash()).to_string()),
                vout: Some(tx_in.prevout().n()),
                script_sig: Some(TransparentScriptSig {
                    // TODO: Implement this
                    //       https://github.com/zcash/wallet/issues/235
                    asm: "TODO: Implement this".into(),
                    hex: script_hex,
                }),
                sequence: tx_in.sequence(),
            }
        }
    }
}

impl TransparentOutput {
    fn encode((tx_out, n): (&TxOut, u16)) -> Self {
        let script_pub_key = TransparentScriptPubKey {
            // TODO: Implement this
            //       https://github.com/zcash/wallet/issues/235
            asm: "TODO: Implement this".into(),
            hex: hex::encode(&tx_out.script_pubkey().0.0),
            // TODO: zcashd relied on initialization behaviour for the default value
            //       for null-data or non-standard outputs. Figure out what it is.
            //       https://github.com/zcash/wallet/issues/236
            req_sigs: 0,
            kind: "nonstandard",
            addresses: vec![],
        };

        Self {
            value: value_from_zatoshis(tx_out.value()),
            value_zat: tx_out.value().into_u64(),
            value_sat: tx_out.value().into_u64(),
            n,
            script_pub_key,
        }
    }
}

#[cfg(zallet_unimplemented)]
impl JoinSplit {
    fn encode(js_desc: &zcash_primitives::transaction::components::sprout::JsDescription) -> Self {
        // https://github.com/zcash/librustzcash/issues/1943
        todo!("Requires zcash_primitives changes")
    }
}

impl SaplingSpend {
    fn encode(spend: &SpendDescription<sapling::bundle::Authorized>) -> Self {
        Self {
            cv: TxId::from_bytes(spend.cv().to_bytes()).to_string(),
            anchor: TxId::from_bytes(spend.anchor().to_bytes()).to_string(),
            nullifier: TxId::from_bytes(spend.nullifier().0).to_string(),
            rk: TxId::from_bytes(<[u8; 32]>::from(*spend.rk())).to_string(),
            proof: hex::encode(spend.zkproof()),
            spend_auth_sig: hex::encode(<[u8; 64]>::from(*spend.spend_auth_sig())),
        }
    }
}

impl SaplingOutput {
    fn encode(output: &OutputDescription<sapling::bundle::GrothProofBytes>) -> Self {
        Self {
            cv: TxId::from_bytes(output.cv().to_bytes()).to_string(),
            cmu: TxId::from_bytes(output.cmu().to_bytes()).to_string(),
            ephemeral_key: TxId::from_bytes(output.ephemeral_key().0).to_string(),
            enc_ciphertext: hex::encode(output.enc_ciphertext()),
            out_ciphertext: hex::encode(output.out_ciphertext()),
            proof: hex::encode(output.zkproof()),
        }
    }
}

impl Orchard {
    fn encode(bundle: Option<&orchard::Bundle<orchard::bundle::Authorized, ZatBalance>>) -> Self {
        match bundle {
            None => Self {
                actions: vec![],
                value_balance: value_from_zat_balance(ZatBalance::zero()),
                value_balance_zat: 0,
                flags: None,
                anchor: None,
                proof: None,
                binding_sig: None,
            },
            Some(bundle) => Self {
                actions: bundle.actions().iter().map(OrchardAction::encode).collect(),
                value_balance: value_from_zat_balance(*bundle.value_balance()),
                value_balance_zat: bundle.value_balance().into(),
                flags: Some(OrchardFlags {
                    enable_spends: bundle.flags().spends_enabled(),
                    enable_outputs: bundle.flags().outputs_enabled(),
                }),
                anchor: Some(hex::encode(bundle.anchor().to_bytes())),
                proof: Some(hex::encode(bundle.authorization().proof())),
                binding_sig: Some(hex::encode(<[u8; 64]>::from(
                    bundle.authorization().binding_signature(),
                ))),
            },
        }
    }
}

impl OrchardAction {
    fn encode(
        action: &orchard::Action<
            orchard::primitives::redpallas::Signature<orchard::primitives::redpallas::SpendAuth>,
        >,
    ) -> Self {
        Self {
            cv: hex::encode(action.cv_net().to_bytes()),
            nullifier: hex::encode(action.nullifier().to_bytes()),
            rk: hex::encode(<[u8; 32]>::from(action.rk())),
            cmx: hex::encode(action.cmx().to_bytes()),
            ephemeral_key: hex::encode(action.encrypted_note().epk_bytes),
            enc_ciphertext: hex::encode(action.encrypted_note().enc_ciphertext),
            out_ciphertext: hex::encode(action.encrypted_note().out_ciphertext),
            spend_auth_sig: hex::encode(<[u8; 64]>::from(action.authorization())),
        }
    }
}
