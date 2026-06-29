use std::convert::Infallible;
use std::num::NonZeroU32;

use abscissa_core::Application;
use jsonrpsee::core::{JsonValue, RpcResult};
use secrecy::ExposeSecret;
use zcash_client_backend::{
    data_api::{
        Account, WalletRead,
        wallet::{
            ConfirmationsPolicy, SpendingKeys, input_selection::GreedyInputSelector,
            propose_transfer,
        },
    },
    fees::{DustOutputPolicy, StandardFeeRule, standard::MultiOutputChangeStrategy},
};
use zcash_keys::keys::UnifiedSpendingKey;
use zcash_protocol::ShieldedProtocol;

use crate::{
    components::{
        chain::Chain,
        database::DbHandle,
        json_rpc::{
            fund_source::{FundSource, FundSourceFilter},
            methods::z_send_many::{build_request, check_orchard_actions_limit, run},
            payments::{AmountParameter, SendResult, enforce_privacy_policy, parse_privacy_policy},
            server::LegacyCode,
            utils::parse_account_parameter,
        },
        keystore::KeyStore,
    },
    prelude::*,
};

#[cfg(feature = "zcashd-import")]
use crate::components::json_rpc::utils::collect_standalone_transparent_keys;

/// Response to a `z_sendfromaccount` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// The result of a `z_sendfromaccount` request: the resulting transaction ID(s).
pub(crate) type ResultType = SendResult;

pub(super) const PARAM_ACCOUNT_DESC: &str = "The UUID of the account to send the funds from.";
pub(super) const PARAM_FUND_SOURCE_DESC: &str = "Where funds may be drawn from: \"orchard\", \"sapling\", \"any_transparent\", or an array \
     of transparent addresses.";
pub(super) const PARAM_RECIPIENTS_DESC: &str =
    "An array of JSON objects representing the amounts to send.";
pub(super) const PARAM_RECIPIENTS_REQUIRED: bool = true;
pub(super) const PARAM_MINCONF_DESC: &str = "Only use funds confirmed at least this many times.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str = "Policy for what information leakage is acceptable, acknowledging the transaction's privacy \
     implications.";

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call<C: Chain>(
    wallet: DbHandle,
    keystore: KeyStore,
    chain: C,
    account: JsonValue,
    fund_source: JsonValue,
    recipients: Vec<AmountParameter>,
    minconf: Option<u32>,
    privacy_policy: String,
) -> Response {
    let request = build_request(&recipients)?;

    let account_id = parse_account_parameter(wallet.as_ref(), &keystore, &account).await?;

    // Fetch the account up front: it both validates that the account exists and provides the
    // key derivation needed to sign the transaction.
    let account = wallet
        .as_ref()
        .get_account(account_id)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or_else(|| {
            LegacyCode::InvalidParameter
                .with_message(format!("No account with UUID {}", account_id.expose_uuid()))
        })?;

    let fund_source = FundSource::parse(&fund_source, wallet.params())?;

    // Unlike `z_proposetransaction`, the caller must explicitly acknowledge the privacy
    // implications of the one-shot send by supplying the privacy policy to enforce.
    let privacy_policy = parse_privacy_policy(Some(&privacy_policy))?;

    let confirmations_policy = match minconf {
        Some(minconf) => NonZeroU32::new(minconf).map_or(
            ConfirmationsPolicy::new_symmetrical(NonZeroU32::MIN, true),
            |c| ConfirmationsPolicy::new_symmetrical(c, false),
        ),
        None => {
            APP.config().builder.confirmations_policy().map_err(|_| {
                LegacyCode::Wallet.with_message(
                    "Configuration error: minimum confirmations for spending trusted TXOs cannot exceed that for untrusted TXOs.")
            })?
        }
    };

    let params = *wallet.params();

    // Propose the transfer with inputs restricted to the requested fund source. The filter
    // borrows the connection immutably; scope it so that borrow is released before we take a
    // mutable borrow to build and sign the transaction.
    let proposal = {
        let change_strategy = MultiOutputChangeStrategy::new(
            StandardFeeRule::Zip317,
            None,
            ShieldedProtocol::Orchard,
            DustOutputPolicy::default(),
            APP.config().note_management.split_policy(),
        );
        let input_selector = GreedyInputSelector::new();
        let mut source = FundSourceFilter::new(wallet.as_ref(), fund_source);

        propose_transfer::<_, _, _, _, Infallible>(
            &mut source,
            &params,
            account_id,
            &input_selector,
            &change_strategy,
            request,
            confirmations_policy,
        )
        // TODO: Map errors to `zcashd` shape.
        .map_err(|e| {
            LegacyCode::Wallet.with_message(format!("Failed to propose transaction: {e}"))
        })?
    };

    enforce_privacy_policy(&proposal, privacy_policy)?;

    check_orchard_actions_limit(&proposal)?;

    let derivation = account.source().key_derivation().ok_or_else(|| {
        LegacyCode::InvalidAddressOrKey
            .with_static("Cannot spend from an account that has no spending key.")
    })?;

    // Fetch the spending key last, to avoid a keystore decryption if unnecessary.
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

    #[cfg(feature = "zcashd-import")]
    let standalone_keys =
        collect_standalone_transparent_keys(wallet.as_ref(), &keystore, account_id, &proposal)
            .await?;

    // Unlike `z_sendmany`, this performs the entire operation in one shot rather than using
    // the background processing system.
    run(
        wallet,
        chain,
        proposal,
        #[cfg(feature = "zcashd-import")]
        SpendingKeys::new(usk, standalone_keys),
        #[cfg(not(feature = "zcashd-import"))]
        SpendingKeys::from_unified_spending_key(usk),
    )
    .await
}
