use jsonrpsee::core::{JsonValue, RpcResult};
use pczt::{
    Pczt,
    roles::{
        prover::Prover,
        signer::{self, Signer},
        updater::Updater,
    },
};
use secrecy::ExposeSecret;
use zcash_client_backend::data_api::{
    Account, WalletRead, wallet::extract_and_store_transaction_from_pczt,
};
use zcash_client_sqlite::ReceivedNoteId;
use zcash_keys::keys::UnifiedSpendingKey;
use zcash_proofs::prover::LocalTxProver;

use crate::components::{
    chain::Chain,
    database::DbHandle,
    json_rpc::{
        payments::{SendResult, broadcast_transactions, parse_privacy_policy},
        server::LegacyCode,
        utils::parse_account_parameter,
    },
    keystore::KeyStore,
};

/// Response to a `z_finalizetransaction` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// The result of a `z_finalizetransaction` request: the resulting transaction ID(s).
pub(crate) type ResultType = SendResult;

pub(super) const PARAM_ACCOUNT_DESC: &str =
    "The UUID of the account whose keys should sign the transaction.";
pub(super) const PARAM_PCZT_DESC: &str =
    "The hex-encoded PCZT to finalize, as returned by z_proposetransaction.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str = "Policy for what information leakage is acceptable, acknowledging the transaction's privacy \
     implications.";

pub(crate) async fn call<C: Chain>(
    wallet: DbHandle,
    keystore: KeyStore,
    chain: C,
    account: JsonValue,
    pczt: String,
    privacy_policy: String,
) -> Response {
    // The caller acknowledges the transaction's privacy implications by supplying the policy
    // that `z_proposetransaction` reported. Validate that it is a known policy.
    //
    // TODO: Once the PCZT's inputs and outputs can be inspected outside the `pczt` crate,
    // re-derive the required policy from the PCZT and reject a weaker acknowledgement here.
    // https://github.com/zcash/wallet/issues/217
    let _privacy_policy = parse_privacy_policy(Some(&privacy_policy))?;

    let pczt = decode_pczt(&pczt)?;

    let account_id = parse_account_parameter(wallet.as_ref(), &keystore, &account).await?;

    let account = wallet
        .as_ref()
        .get_account(account_id)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or_else(|| {
            LegacyCode::InvalidParameter
                .with_message(format!("No account with UUID {}", account_id.expose_uuid()))
        })?;

    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::InvalidAddressOrKey
            .with_static("Cannot sign for an account that has no spending key.")
    })?;

    let seed = keystore
        .decrypt_seed(derivation.seed_fingerprint())
        .await
        .map_err(|e| match e.kind() {
            crate::error::ErrorKind::Generic if e.to_string() == "Wallet is locked" => {
                LegacyCode::WalletUnlockNeeded.with_message(e.to_string())
            }
            _ => LegacyCode::Database.with_message(e.to_string()),
        })?;
    let usk = UnifiedSpendingKey::from_seed(
        wallet.params(),
        seed.expose_secret(),
        derivation.account_index(),
    )
    .map_err(|e| LegacyCode::InvalidAddressOrKey.with_message(e.to_string()))?;

    // Proving, signing, and proof verification are CPU-bound; run them on the blocking pool.
    let (wallet, txid) = crate::spawn_blocking!("z_finalizetransaction prover", move || {
        let pczt = authorize_pczt(pczt, &usk)?;

        let prover = LocalTxProver::bundled();
        let (spend_vk, output_vk) = prover.verifying_keys();
        let orchard_vk = orchard::circuit::VerifyingKey::build();

        let mut wallet = wallet;
        let txid = extract_and_store_transaction_from_pczt::<_, ReceivedNoteId>(
            wallet.as_mut(),
            pczt,
            Some((&spend_vk, &output_vk)),
            Some(&orchard_vk),
        )
        .map_err(|e| {
            LegacyCode::Wallet.with_message(format!("Failed to extract transaction from PCZT: {e}"))
        })?;

        Ok::<_, jsonrpsee::types::ErrorObjectOwned>((wallet, txid))
    })
    .await
    .map_err(|e| {
        LegacyCode::Wallet.with_message(format!("PCZT finalization task panicked: {e}"))
    })??;

    broadcast_transactions(&wallet, chain, vec![txid]).await
}

/// Decodes the hex-encoded PCZT argument into a [`Pczt`], mapping malformed input to a
/// JSON-RPC invalid-parameter error.
fn decode_pczt(pczt: &str) -> RpcResult<Pczt> {
    let bytes = hex::decode(pczt.trim())
        .map_err(|e| LegacyCode::InvalidParameter.with_message(format!("Invalid PCZT hex: {e}")))?;
    Pczt::parse(&bytes)
        .map_err(|e| LegacyCode::InvalidParameter.with_message(format!("Invalid PCZT: {e:?}")))
}

/// Adds proof generation keys, creates proofs, and applies this account's spend authorizing
/// signatures to a PCZT, returning the fully-authorized PCZT ready for extraction.
///
/// This handles fully-shielded (Sapling and Orchard) PCZTs. The spend metadata does not say
/// which spends belong to this account, so each candidate signature is attempted and
/// wrong-key errors are ignored, matching the reference driver in `zcash_client_backend`.
///
/// PCZTs containing transparent inputs are not yet finalized here: the `pczt` crate does not
/// expose the per-input derivation needed to derive the transparent signing keys. Such a PCZT
/// fails at extraction rather than being broadcast unsigned.
fn authorize_pczt(pczt: Pczt, usk: &UnifiedSpendingKey) -> RpcResult<Pczt> {
    let sapling_extsk = usk.sapling();

    // 1. Add Sapling proof generation keys to the account's (non-dummy) spends. Orchard has
    //    no equivalent step.
    let pczt = Updater::new(pczt)
        .update_sapling_with(|mut updater| {
            let spends_without_pgk = updater
                .bundle()
                .spends()
                .iter()
                .enumerate()
                .filter_map(|(index, spend)| {
                    spend.proof_generation_key().is_none().then_some(index)
                })
                .collect::<Vec<_>>();

            for index in spends_without_pgk {
                updater.update_spend_with(index, |mut spend_updater| {
                    spend_updater
                        .set_proof_generation_key(sapling_extsk.expsk.proof_generation_key())
                })?;
            }

            Ok(())
        })
        .map_err(|e| {
            LegacyCode::Wallet.with_message(format!(
                "Failed to update PCZT with proof generation keys: {e:?}"
            ))
        })?
        .finish();

    // 2. Create proofs, building each (expensive) proving key only when the PCZT needs it.
    let prover = Prover::new(pczt);
    let prover = if prover.requires_sapling_proofs() {
        let sapling_prover = LocalTxProver::bundled();
        prover
            .create_sapling_proofs(&sapling_prover, &sapling_prover)
            .map_err(|e| {
                LegacyCode::Wallet.with_message(format!("Failed to create Sapling proofs: {e:?}"))
            })?
    } else {
        prover
    };
    let prover = if prover.requires_orchard_proof() {
        let orchard_pk = orchard::circuit::ProvingKey::build();
        prover.create_orchard_proof(&orchard_pk).map_err(|e| {
            LegacyCode::Wallet.with_message(format!("Failed to create Orchard proof: {e:?}"))
        })?
    } else {
        prover
    };
    let pczt = prover.finish();

    // 3. Apply spend authorizing signatures for both shielded pools.
    let mut signer = Signer::new(pczt).map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Failed to start PCZT signer: {e:?}"))
    })?;

    let sapling_ask = &sapling_extsk.expsk.ask;
    for index in 0.. {
        match signer.sign_sapling(index, sapling_ask) {
            Err(signer::Error::InvalidIndex) => break,
            Ok(())
            | Err(signer::Error::SaplingSign(
                sapling::pczt::SignerError::WrongSpendAuthorizingKey,
            )) => {}
            Err(e) => {
                return Err(LegacyCode::Wallet
                    .with_message(format!("Failed to apply Sapling signature: {e:?}")));
            }
        }
    }

    let orchard_ask = orchard::keys::SpendAuthorizingKey::from(usk.orchard());
    for index in 0.. {
        match signer.sign_orchard(index, &orchard_ask) {
            Err(signer::Error::InvalidIndex) => break,
            Ok(())
            | Err(signer::Error::OrchardSign(
                orchard::pczt::SignerError::WrongSpendAuthorizingKey,
            )) => {}
            Err(e) => {
                return Err(LegacyCode::Wallet
                    .with_message(format!("Failed to apply Orchard signature: {e:?}")));
            }
        }
    }

    Ok(signer.finish())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn rejects_non_hex_input() {
        let err = decode_pczt("nothex").expect_err("non-hex PCZT should be rejected");
        assert!(
            err.message().starts_with("Invalid PCZT hex:"),
            "unexpected message: {}",
            err.message(),
        );
    }

    #[test]
    fn rejects_valid_hex_that_is_not_a_pczt() {
        // Valid hex, but not a PCZT (wrong magic bytes / structure).
        let err = decode_pczt("00010203").expect_err("non-PCZT bytes should be rejected");
        assert!(
            err.message().starts_with("Invalid PCZT:"),
            "unexpected message: {}",
            err.message(),
        );
    }

    #[test]
    fn ignores_surrounding_whitespace() {
        // Whitespace is trimmed before decoding, so the error is about the PCZT contents,
        // not the hex.
        let err = decode_pczt("  00  ").expect_err("non-PCZT bytes should be rejected");
        assert!(err.message().starts_with("Invalid PCZT:"));
    }

    proptest! {
        /// Decoding never panics, whatever the caller passes.
        #[test]
        fn never_panics_on_arbitrary_strings(s in ".*") {
            let _ = decode_pczt(&s);
        }

        /// Arbitrary well-formed hex that is not a real PCZT is rejected cleanly (never
        /// parses, never panics).
        #[test]
        fn rejects_arbitrary_hex_bytes(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
            let err = decode_pczt(&hex::encode(&bytes))
                .expect_err("random bytes are not a valid PCZT");
            prop_assert!(err.message().starts_with("Invalid PCZT:"));
        }
    }
}
