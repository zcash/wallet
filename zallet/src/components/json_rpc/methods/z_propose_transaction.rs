use std::convert::Infallible;
use std::num::NonZeroU32;

use abscissa_core::Application;
use documented::Documented;
use jsonrpsee::core::{JsonValue, RpcResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zcash_client_backend::{
    data_api::{
        WalletRead,
        wallet::{
            ConfirmationsPolicy, create_pczt_from_proposal, input_selection::GreedyInputSelector,
            propose_transfer,
        },
    },
    fees::{DustOutputPolicy, StandardFeeRule, standard::MultiOutputChangeStrategy},
    wallet::OvkPolicy,
};
use zcash_protocol::ShieldedProtocol;

use crate::{
    components::{
        database::DbHandle,
        json_rpc::{
            fund_source::{FundSource, FundSourceFilter},
            methods::z_send_many::build_request,
            payments::{AmountParameter, required_privacy_policy},
            server::LegacyCode,
            utils::parse_account_parameter,
        },
        keystore::KeyStore,
    },
    prelude::*,
};

/// Response to a `z_proposetransaction` RPC request.
pub(crate) type Response = RpcResult<ResultType>;

/// A proposed transaction, returned for inspection before it is finalized.
#[derive(Clone, Debug, Serialize, Deserialize, Documented, JsonSchema)]
pub(crate) struct ResultType {
    /// The proposed transaction as a hex-encoded PCZT.
    ///
    /// This can be inspected to review the transaction's effects, and later passed to
    /// `z_finalizetransaction` to sign and broadcast it.
    pczt: String,

    /// The privacy policy required to execute this transaction.
    ///
    /// This is the strictest policy that permits the proposed transaction; it must be
    /// supplied to `z_finalizetransaction` as acknowledgement of the transaction's privacy
    /// implications.
    privacy_policy: String,
}

pub(super) const PARAM_ACCOUNT_DESC: &str = "The UUID of the account to send the funds from.";
pub(super) const PARAM_FUND_SOURCE_DESC: &str = "Where funds may be drawn from: \"orchard\", \"sapling\", \"any_transparent\", or an array \
     of transparent addresses.";
pub(super) const PARAM_RECIPIENTS_DESC: &str =
    "An array of JSON objects representing the amounts to send.";
pub(super) const PARAM_RECIPIENTS_REQUIRED: bool = true;
pub(super) const PARAM_MINCONF_DESC: &str = "Only use funds confirmed at least this many times.";
pub(super) const PARAM_PRIVACY_POLICY_DESC: &str =
    "Policy for what information leakage is acceptable.";

pub(crate) async fn call(
    mut wallet: DbHandle,
    keystore: KeyStore,
    account: JsonValue,
    fund_source: JsonValue,
    recipients: Vec<AmountParameter>,
    minconf: Option<u32>,
    privacy_policy: Option<String>,
) -> Response {
    let request = build_request(&recipients)?;

    let account_id = parse_account_parameter(wallet.as_ref(), &keystore, &account).await?;

    // Validate that the account exists before proposing, for a clear error.
    if wallet
        .as_ref()
        .get_account(account_id)
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .is_none()
    {
        return Err(LegacyCode::InvalidParameter
            .with_message(format!("No account with UUID {}", account_id.expose_uuid())));
    }

    let fund_source = FundSource::parse(&fund_source, wallet.params())?;

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
    // borrows the connection immutably; scope it so that borrow is released before we take
    // a mutable borrow to build the PCZT.
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

    let privacy_policy = required_privacy_policy(&proposal);

    // Build the PCZT from the proposal. This does not touch spending keys and does not
    // generate proofs; both are deferred to `z_finalizetransaction`.
    let pczt = create_pczt_from_proposal::<_, _, Infallible, _, Infallible, _>(
        wallet.as_mut(),
        &params,
        account_id,
        OvkPolicy::Sender,
        &proposal,
    )
    .map_err(|e| LegacyCode::Wallet.with_message(format!("Failed to create PCZT: {e}")))?;

    Ok(ResultType {
        pczt: hex::encode(pczt.serialize()),
        privacy_policy: privacy_policy.to_string(),
    })
}
