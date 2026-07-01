//! Restricting transaction inputs to a caller-specified source.
//!
//! `z_proposetransaction` / `z_sendfromaccount` accept a `fund_source` argument naming
//! where an account's funds may be drawn from. The standard `propose_transfer` +
//! [`GreedyInputSelector`] path in `zcash_client_backend` 0.23 offers no caller-facing
//! restriction on which value pools or transparent addresses are spent from: the selector
//! computes the eligible pools from the target transaction version and passes them to
//! [`InputSource::select_spendable_notes`].
//!
//! [`FundSourceFilter`] is a thin wrapper around [`DbConnection`] that implements
//! [`InputSource`] (and forwards [`WalletRead`]) while narrowing the eligible pools and
//! transparent addresses to the requested [`FundSource`]. Because all of the trait methods
//! used by `propose_transfer` take `&self`, the wrapper can hold a shared `&DbConnection`.
//!
//! [`GreedyInputSelector`]: zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelector

use std::collections::{BTreeMap, HashMap, HashSet};

use jsonrpsee::core::{JsonValue, RpcResult};
use transparent::{
    address::TransparentAddress,
    bundle::{OutPoint, TxOut},
};
use zcash_client_backend::{
    data_api::{
        AccountMeta, InputSource, NoteFilter, ReceivedNotes, TargetValue, TransparentOutputFilter,
        WalletRead,
        wallet::{ConfirmationsPolicy, TargetHeight},
    },
    fees::{StandardFeeRule, TransactionBalance},
    proposal::Proposal,
    wallet::{Note, ReceivedNote, WalletTransparentOutput},
    zip321::{Payment, TransactionRequest},
};
use zcash_client_sqlite::{AccountUuid, ReceivedNoteId};
use zcash_keys::address::Address;
use zcash_primitives::transaction::fees::{
    FeeRule,
    transparent::{InputSize, InputView, OutputView},
    zip317::P2PKH_STANDARD_OUTPUT_SIZE,
};
use zcash_protocol::{PoolType, ShieldedProtocol, TxId, consensus::BlockHeight, value::Zatoshis};

use crate::{components::database::DbConnection, network::Network};

use super::server::LegacyCode;

/// Change below this value is folded into the fee rather than creating a separate transparent
/// change output (creating one would cost roughly this much in fee to later spend).
const TRANSPARENT_DUST_THRESHOLD: Zatoshis = Zatoshis::const_from_u64(5_000);

/// Where an account's funds may be drawn from when constructing a transaction.
#[derive(Clone, Debug)]
pub(super) enum FundSource {
    /// Spend only Orchard notes.
    Orchard,
    /// Spend only Sapling notes.
    Sapling,
    /// Spend any of the account's transparent funds.
    AnyTransparent,
    /// Spend only transparent funds received at the given transparent addresses.
    Transparent(HashSet<TransparentAddress>),
}

impl FundSource {
    /// Parses a `fund_source` JSON-RPC argument.
    ///
    /// Accepts either one of the strings `"orchard"`, `"sapling"`, `"any_transparent"`, or
    /// an array of transparent address strings.
    pub(super) fn parse(value: &JsonValue, params: &Network) -> RpcResult<Self> {
        match value {
            JsonValue::String(s) => match s.as_str() {
                "orchard" => Ok(Self::Orchard),
                "sapling" => Ok(Self::Sapling),
                "any_transparent" => Ok(Self::AnyTransparent),
                other => Err(LegacyCode::InvalidParameter.with_message(format!(
                    "Invalid fund_source: expected \"orchard\", \"sapling\", \"any_transparent\", \
                     or an array of transparent addresses, got \"{other}\"."
                ))),
            },
            JsonValue::Array(addrs) => {
                if addrs.is_empty() {
                    return Err(LegacyCode::InvalidParameter.with_static(
                        "Invalid fund_source: the array of transparent addresses is empty.",
                    ));
                }
                let mut set = HashSet::new();
                for addr in addrs {
                    let s = addr.as_str().ok_or_else(|| {
                        LegacyCode::InvalidParameter.with_static(
                            "Invalid fund_source: array entries must be transparent address \
                             strings.",
                        )
                    })?;
                    match Address::decode(params, s) {
                        Some(Address::Transparent(ta)) => {
                            set.insert(ta);
                        }
                        _ => {
                            return Err(LegacyCode::InvalidParameter.with_message(format!(
                                "Invalid fund_source: \"{s}\" is not a transparent address."
                            )));
                        }
                    }
                }
                Ok(Self::Transparent(set))
            }
            _ => Err(LegacyCode::InvalidParameter.with_static(
                "Invalid fund_source: expected a string or an array of transparent addresses.",
            )),
        }
    }

    /// The shielded pools this source permits spending from.
    fn allowed_pools(&self) -> &'static [ShieldedProtocol] {
        match self {
            Self::Orchard => &[ShieldedProtocol::Orchard],
            Self::Sapling => &[ShieldedProtocol::Sapling],
            Self::AnyTransparent | Self::Transparent(_) => &[],
        }
    }

    /// Whether this source permits spending transparent funds received at `address`.
    fn allows_taddr(&self, address: &TransparentAddress) -> bool {
        match self {
            Self::Orchard | Self::Sapling => false,
            Self::AnyTransparent => true,
            Self::Transparent(set) => set.contains(address),
        }
    }
}

/// Builds a single-step proposal that spends the account's transparent UTXOs (restricted to
/// `source`) to the recipients in `request`, with transparent change.
///
/// `zcash_client_backend`'s `propose_transfer` selects only shielded notes; its transparent
/// input handling lives exclusively in the shielding path (which sweeps to the account's own
/// shielded pool). To spend transparent funds to arbitrary transparent recipients without
/// shielding, this constructs the proposal directly: it gathers spendable UTXOs, greedily
/// selects enough to cover the payments plus the ZIP 317 fee, and returns any change to the
/// account as an ordinary transparent output (single-step proposals cannot carry an ephemeral
/// change output). The resulting proposal flows through the same `create_pczt_from_proposal` /
/// `create_proposed_transactions` path as shielded proposals, and `z_finalizetransaction`
/// already signs transparent inputs.
///
/// Only transparent recipients are supported here; a shielded recipient on a transparent fund
/// source is rejected, because it would reveal the transparent senders and is better expressed
/// as a shielding operation.
pub(super) fn propose_transparent_spend(
    wallet: &DbConnection,
    account_id: AccountUuid,
    source: &FundSource,
    request: TransactionRequest,
    confirmations_policy: ConfirmationsPolicy,
) -> RpcResult<Proposal<StandardFeeRule, ReceivedNoteId>> {
    let params = *wallet.params();
    let overflow = || LegacyCode::InvalidParameter.with_static("Transaction value overflow");

    let (target_height, _anchor_height) = wallet
        .get_target_and_anchor_heights(confirmations_policy.trusted())
        .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
        .ok_or_else(|| LegacyCode::InWarmup.with_static("Wallet sync required"))?;

    // Resolve the payments to transparent outputs, recording the pool of each (always
    // transparent) and collecting `TxOut`s for fee sizing.
    let mut payment_outputs: Vec<TxOut> = vec![];
    let mut total_payments = Zatoshis::ZERO;
    for payment in request.payments().values() {
        let amount = payment.amount().ok_or_else(|| {
            LegacyCode::InvalidParameter.with_static("Payment is missing an amount")
        })?;
        let address = Address::try_from_zcash_address(&params, payment.recipient_address().clone())
            .map_err(|e| LegacyCode::InvalidAddressOrKey.with_message(e.to_string()))?;
        let ta = match address {
            Address::Transparent(ta) => ta,
            _ => {
                return Err(LegacyCode::InvalidParameter.with_static(
                    "A transparent fund source supports only transparent recipients; \
                     use a shielded fund source to send to shielded addresses.",
                ));
            }
        };
        payment_outputs.push(TxOut::new(amount, ta.script().into()));
        total_payments = (total_payments + amount).ok_or_else(overflow)?;
    }
    let payment_output_sizes: Vec<usize> = payment_outputs
        .iter()
        .map(OutputView::serialized_size)
        .collect();

    // Gather the candidate UTXOs from the addresses the fund source permits.
    let addresses: Vec<TransparentAddress> = match source {
        FundSource::Transparent(set) => set.iter().copied().collect(),
        FundSource::AnyTransparent => wallet
            .get_transparent_receivers(account_id, true, true)
            .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?
            .into_keys()
            .collect(),
        FundSource::Orchard | FundSource::Sapling => {
            return Err(LegacyCode::Wallet
                .with_static("propose_transparent_spend called with a shielded fund source"));
        }
    };
    let mut utxos: Vec<WalletTransparentOutput<AccountUuid>> = vec![];
    for address in addresses {
        utxos.extend(
            wallet
                .get_spendable_transparent_outputs(
                    &address,
                    target_height,
                    confirmations_policy,
                    TransparentOutputFilter::All,
                )
                .map_err(|e| LegacyCode::Database.with_message(e.to_string()))?,
        );
    }
    // Greedily prefer larger UTXOs to minimize the input count (and thus the fee).
    utxos.sort_by_key(|u| core::cmp::Reverse(u.value()));

    // Select inputs to cover the payments plus the ZIP 317 fee, with transparent change.
    let utxo_values: Vec<(Zatoshis, InputSize)> = utxos
        .iter()
        .map(|u| (u.value(), u.serialized_size()))
        .collect();
    let plan = plan_transparent_spend(&params, &utxo_values, &payment_output_sizes, total_payments)
        .map_err(|e| match e {
            SpendPlanError::Insufficient { have, need } => {
                LegacyCode::Wallet.with_message(format!(
                    "Failed to propose transaction: Insufficient balance (have {}, need {} \
                 including fee)",
                    u64::from(have),
                    u64::from(need),
                ))
            }
            SpendPlanError::Overflow => overflow(),
            SpendPlanError::Fee(msg) => {
                LegacyCode::Wallet.with_message(format!("Fee calculation failed: {msg}"))
            }
        })?;

    // Build the final output set: the requested payments plus, if there is change, a real
    // transparent change output back to the account. Single-step proposals cannot carry an
    // ephemeral change output, so change is an ordinary output rather than a `ChangeValue`.
    let mut payments: Vec<Payment> = request.payments().values().cloned().collect();
    if let Some(&change) = plan.change.first() {
        let change_address = *utxos
            .first()
            .expect("a non-zero change implies at least one selected input")
            .recipient_address();
        payments.push(
            Payment::new(
                Address::Transparent(change_address).to_zcash_address(&params),
                Some(change),
                None,
                None,
                None,
                vec![],
            )
            .map_err(|_| LegacyCode::Wallet.with_static("Failed to construct change output"))?,
        );
    }
    let payment_pools: BTreeMap<usize, PoolType> = (0..payments.len())
        .map(|i| (i, PoolType::TRANSPARENT))
        .collect();
    let request = TransactionRequest::new(payments).map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Invalid transaction request: {e}"))
    })?;
    let balance = TransactionBalance::new(vec![], plan.fee).map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Invalid transaction balance: {e:?}"))
    })?;

    // Drop the account id; the proposal is account-agnostic.
    let transparent_inputs: Vec<WalletTransparentOutput<()>> = utxos[..plan.n_selected]
        .iter()
        .map(|u| {
            WalletTransparentOutput::from_parts(
                u.outpoint().clone(),
                u.txout().clone(),
                u.mined_height(),
                u.recipient_account().map(|_| ()),
                u.recipient_key_scope(),
                None,
            )
            .expect("a spendable wallet UTXO reconstructs into a valid transparent input")
        })
        .collect();

    Proposal::single_step(
        request,
        payment_pools,
        transparent_inputs,
        None,
        balance,
        StandardFeeRule::Zip317,
        target_height,
        false,
    )
    .map_err(|e| {
        LegacyCode::Wallet.with_message(format!("Failed to build transparent proposal: {e:?}"))
    })
}

/// The result of [`plan_transparent_spend`]: how many of the (largest-first) UTXOs to select,
/// the fee, and the transparent change outputs to add.
struct TransparentSpendPlan {
    n_selected: usize,
    fee: Zatoshis,
    change: Vec<Zatoshis>,
}

/// Why a transparent spend could not be planned.
#[derive(Debug)]
enum SpendPlanError {
    /// The available UTXOs cannot cover the payments plus the fee.
    Insufficient { have: Zatoshis, need: Zatoshis },
    /// The ZIP 317 fee calculation failed.
    Fee(String),
    /// A value computation overflowed.
    Overflow,
}

/// Greedily selects transparent inputs from `utxos` (which must be sorted largest-first) to
/// cover `total_payments` plus the ZIP 317 fee for `payment_output_sizes`, deciding whether the
/// remainder is worth a transparent change output.
///
/// Pure (depends only on values and ZIP 317 output sizing; `params` and the height do not affect
/// the ZIP 317 fee), so it is unit-testable without a wallet. The returned `(fee, change)`
/// always satisfies `Σselected = total_payments + Σchange + fee` exactly, as
/// [`Proposal::single_step`] requires.
fn plan_transparent_spend<P: zcash_protocol::consensus::Parameters>(
    params: &P,
    utxos: &[(Zatoshis, InputSize)],
    payment_output_sizes: &[usize],
    total_payments: Zatoshis,
) -> Result<TransparentSpendPlan, SpendPlanError> {
    let fee_rule = StandardFeeRule::Zip317;
    // ZIP 317 ignores the height; any value works.
    let height = BlockHeight::from_u32(0);
    let fee_for = |input_sizes: &[InputSize], output_sizes: &[usize]| {
        fee_rule
            .fee_required(
                params,
                height,
                input_sizes.iter().cloned(),
                output_sizes.iter().copied(),
                0,
                0,
                0,
                0,
            )
            .map_err(|e| SpendPlanError::Fee(format!("{e:?}")))
    };

    let mut input_total = Zatoshis::ZERO;
    let mut last_required = total_payments;

    for n in 0..utxos.len() {
        input_total = (input_total + utxos[n].0).ok_or(SpendPlanError::Overflow)?;
        let input_sizes: Vec<InputSize> = utxos[..=n].iter().map(|(_, s)| s.clone()).collect();

        let fee_no_change = fee_for(&input_sizes, payment_output_sizes)?;
        last_required = (total_payments + fee_no_change).ok_or(SpendPlanError::Overflow)?;
        if input_total < last_required {
            continue;
        }

        // We can cover the change-free transaction. A change output is only worth creating if
        // what remains after paying the (higher) fee for it clears the dust threshold.
        let mut out_sizes = payment_output_sizes.to_vec();
        out_sizes.push(P2PKH_STANDARD_OUTPUT_SIZE);
        let fee_with_change = fee_for(&input_sizes, &out_sizes)?;

        let change = (total_payments + fee_with_change).and_then(|need| input_total - need);
        return Ok(match change {
            Some(change) if change >= TRANSPARENT_DUST_THRESHOLD => TransparentSpendPlan {
                n_selected: n + 1,
                fee: fee_with_change,
                change: vec![change],
            },
            // Change is dust (or a change output is unaffordable): omit it and fold the surplus
            // into the fee. `input_total >= total_payments` holds here.
            _ => TransparentSpendPlan {
                n_selected: n + 1,
                fee: (input_total - total_payments).expect("input_total covers the payments"),
                change: vec![],
            },
        });
    }

    Err(SpendPlanError::Insufficient {
        have: input_total,
        need: last_required,
    })
}

/// A [`DbConnection`] view that restricts input selection to a [`FundSource`].
///
/// Implements [`InputSource`] by intersecting the eligible shielded pools and transparent
/// addresses with the wrapped [`FundSource`], and forwards [`WalletRead`] unchanged. Used
/// as the `wallet_db` and change-strategy `MetaSource` for `propose_transfer`.
pub(super) struct FundSourceFilter<'a> {
    inner: &'a DbConnection,
    source: FundSource,
}

impl<'a> FundSourceFilter<'a> {
    pub(super) fn new(inner: &'a DbConnection, source: FundSource) -> Self {
        Self { inner, source }
    }

    /// Returns the requested `sources` intersected with the pools this fund source allows.
    fn restrict_pools(&self, sources: &[ShieldedProtocol]) -> Vec<ShieldedProtocol> {
        let allowed = self.source.allowed_pools();
        sources
            .iter()
            .copied()
            .filter(|p| allowed.contains(p))
            .collect()
    }
}

impl InputSource for FundSourceFilter<'_> {
    type Error = <DbConnection as InputSource>::Error;
    type AccountId = <DbConnection as InputSource>::AccountId;
    type NoteRef = <DbConnection as InputSource>::NoteRef;

    fn get_spendable_note(
        &self,
        txid: &TxId,
        protocol: ShieldedProtocol,
        index: u32,
        target_height: TargetHeight,
    ) -> Result<Option<ReceivedNote<Self::NoteRef, Note>>, Self::Error> {
        self.inner
            .get_spendable_note(txid, protocol, index, target_height)
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: TargetValue,
        sources: &[ShieldedProtocol],
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        exclude: &[Self::NoteRef],
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        self.inner.select_spendable_notes(
            account,
            target_value,
            &self.restrict_pools(sources),
            target_height,
            confirmations_policy,
            exclude,
        )
    }

    fn select_unspent_notes(
        &self,
        account: Self::AccountId,
        sources: &[ShieldedProtocol],
        target_height: TargetHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<ReceivedNotes<Self::NoteRef>, Self::Error> {
        self.inner.select_unspent_notes(
            account,
            &self.restrict_pools(sources),
            target_height,
            exclude,
        )
    }

    fn get_unspent_transparent_output(
        &self,
        outpoint: &OutPoint,
        target_height: TargetHeight,
    ) -> Result<Option<WalletTransparentOutput<Self::AccountId>>, Self::Error> {
        Ok(self
            .inner
            .get_unspent_transparent_output(outpoint, target_height)?
            .filter(|output| self.source.allows_taddr(output.recipient_address())))
    }

    fn get_spendable_transparent_outputs(
        &self,
        address: &TransparentAddress,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        output_filter: TransparentOutputFilter,
    ) -> Result<Vec<WalletTransparentOutput<Self::AccountId>>, Self::Error> {
        if self.source.allows_taddr(address) {
            self.inner.get_spendable_transparent_outputs(
                address,
                target_height,
                confirmations_policy,
                output_filter,
            )
        } else {
            Ok(vec![])
        }
    }

    fn get_account_metadata(
        &self,
        account: Self::AccountId,
        selector: &NoteFilter,
        target_height: TargetHeight,
        exclude: &[Self::NoteRef],
    ) -> Result<AccountMeta, Self::Error> {
        self.inner
            .get_account_metadata(account, selector, target_height, exclude)
    }
}

impl WalletRead for FundSourceFilter<'_> {
    type Error = <DbConnection as WalletRead>::Error;
    type AccountId = <DbConnection as WalletRead>::AccountId;
    type Account = <DbConnection as WalletRead>::Account;

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        self.inner.get_account_ids()
    }

    fn get_account(
        &self,
        account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        self.inner.get_account(account_id)
    }

    fn get_derived_account(
        &self,
        derivation: &zcash_client_backend::data_api::Zip32Derivation,
    ) -> Result<Option<Self::Account>, Self::Error> {
        self.inner.get_derived_account(derivation)
    }

    fn validate_seed(
        &self,
        account_id: Self::AccountId,
        seed: &secrecy::SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        self.inner.validate_seed(account_id, seed)
    }

    fn seed_relevance_to_derived_accounts(
        &self,
        seed: &secrecy::SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        self.inner.seed_relevance_to_derived_accounts(seed)
    }

    fn get_account_for_ufvk(
        &self,
        ufvk: &zcash_keys::keys::UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        self.inner.get_account_for_ufvk(ufvk)
    }

    fn list_addresses(
        &self,
        account: Self::AccountId,
    ) -> Result<Vec<zcash_client_backend::data_api::AddressInfo>, Self::Error> {
        self.inner.list_addresses(account)
    }

    fn get_last_generated_address_matching(
        &self,
        account: Self::AccountId,
        address_filter: zcash_client_backend::keys::UnifiedAddressRequest,
    ) -> Result<Option<zcash_client_backend::address::UnifiedAddress>, Self::Error> {
        self.inner
            .get_last_generated_address_matching(account, address_filter)
    }

    fn get_account_birthday(&self, account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        self.inner.get_account_birthday(account)
    }

    fn get_wallet_birthday(&self) -> Result<Option<BlockHeight>, Self::Error> {
        self.inner.get_wallet_birthday()
    }

    fn get_wallet_summary(
        &self,
        confirmations_policy: ConfirmationsPolicy,
    ) -> Result<Option<zcash_client_backend::data_api::WalletSummary<Self::AccountId>>, Self::Error>
    {
        self.inner.get_wallet_summary(confirmations_policy)
    }

    fn chain_height(&self) -> Result<Option<BlockHeight>, Self::Error> {
        self.inner.chain_height()
    }

    fn get_block_hash(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<zcash_primitives::block::BlockHash>, Self::Error> {
        self.inner.get_block_hash(block_height)
    }

    fn block_metadata(
        &self,
        height: BlockHeight,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        self.inner.block_metadata(height)
    }

    fn block_fully_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        self.inner.block_fully_scanned()
    }

    fn get_max_height_hash(
        &self,
    ) -> Result<Option<(BlockHeight, zcash_primitives::block::BlockHash)>, Self::Error> {
        self.inner.get_max_height_hash()
    }

    fn block_max_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        self.inner.block_max_scanned()
    }

    fn suggest_scan_ranges(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
        self.inner.suggest_scan_ranges()
    }

    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: std::num::NonZeroU32,
    ) -> Result<Option<(TargetHeight, BlockHeight)>, Self::Error> {
        self.inner.get_target_and_anchor_heights(min_confirmations)
    }

    fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        self.inner.get_tx_height(txid)
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<Self::AccountId, zcash_keys::keys::UnifiedFullViewingKey>, Self::Error>
    {
        self.inner.get_unified_full_viewing_keys()
    }

    fn get_memo(
        &self,
        note_id: zcash_client_backend::wallet::NoteId,
    ) -> Result<Option<zcash_protocol::memo::Memo>, Self::Error> {
        self.inner.get_memo(note_id)
    }

    fn get_transaction(
        &self,
        txid: TxId,
    ) -> Result<Option<zcash_primitives::transaction::Transaction>, Self::Error> {
        self.inner.get_transaction(txid)
    }

    fn get_sapling_nullifiers(
        &self,
        query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling::Nullifier)>, Self::Error> {
        self.inner.get_sapling_nullifiers(query)
    }

    fn get_orchard_nullifiers(
        &self,
        query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        self.inner.get_orchard_nullifiers(query)
    }

    fn get_transparent_receivers(
        &self,
        account: Self::AccountId,
        include_change: bool,
        include_standalone: bool,
    ) -> Result<
        HashMap<TransparentAddress, zcash_client_backend::wallet::TransparentAddressMetadata>,
        Self::Error,
    > {
        self.inner
            .get_transparent_receivers(account, include_change, include_standalone)
    }

    fn get_ephemeral_transparent_receivers(
        &self,
        account: Self::AccountId,
        exposure_depth: u32,
        exclude_used: bool,
    ) -> Result<
        HashMap<TransparentAddress, zcash_client_backend::wallet::TransparentAddressMetadata>,
        Self::Error,
    > {
        self.inner
            .get_ephemeral_transparent_receivers(account, exposure_depth, exclude_used)
    }

    fn get_transparent_balances(
        &self,
        account: Self::AccountId,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
    ) -> Result<
        HashMap<
            TransparentAddress,
            (
                zcash_client_backend::data_api::TransparentKeyOrigin,
                zcash_client_backend::data_api::Balance,
            ),
        >,
        Self::Error,
    > {
        self.inner
            .get_transparent_balances(account, target_height, confirmations_policy)
    }

    fn get_transparent_address_metadata(
        &self,
        account: Self::AccountId,
        address: &TransparentAddress,
    ) -> Result<Option<zcash_client_backend::wallet::TransparentAddressMetadata>, Self::Error> {
        self.inner
            .get_transparent_address_metadata(account, address)
    }

    fn utxo_query_height(&self, account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        self.inner.utxo_query_height(account)
    }

    fn transaction_data_requests(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::TransactionDataRequest>, Self::Error> {
        self.inner.transaction_data_requests()
    }

    fn get_received_outputs(
        &self,
        txid: TxId,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
    ) -> Result<Vec<zcash_client_backend::data_api::ReceivedTransactionOutput>, Self::Error> {
        self.inner
            .get_received_outputs(txid, target_height, confirmations_policy)
    }

    fn find_account_for_address<P: zcash_protocol::consensus::Parameters>(
        &self,
        params: &P,
        address: &Address,
    ) -> Result<
        Option<Self::AccountId>,
        zcash_client_backend::data_api::error::FindAccountForAddressError<Self::Error>,
    > {
        self.inner.find_account_for_address(params, address)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;
    use zcash_protocol::consensus;

    use super::*;

    fn mainnet() -> Network {
        Network::Consensus(consensus::Network::MainNetwork)
    }

    // Transparent addresses reused from the `validate_address` / `verify_message` tests.
    const MAINNET_P2PKH: &str = "t1VydNnkjBzfL1iAMyUbwGKJAF7PgvuCfMY";
    const MAINNET_P2SH: &str = "t3Vz22vK5z2LcKEdg16Yv4FFneEL1zg9ojd";

    // A Sapling shielded address; valid, but not a transparent address.
    const MAINNET_SAPLING: &str =
        "zs1qqqqqqqqqqqqqqqqqqcguyvaw2vjk4sdyeg0lc970u659lvhqq7t0np6hlup5lusxle75c8v35z";

    /// Decodes a known-transparent address string into a [`TransparentAddress`].
    fn taddr(s: &str) -> TransparentAddress {
        match Address::decode(&mainnet(), s) {
            Some(Address::Transparent(ta)) => ta,
            _ => panic!("{s} is not a transparent address"),
        }
    }

    /// Extracts the error message from a parse failure.
    fn parse_err(value: JsonValue) -> String {
        FundSource::parse(&value, &mainnet())
            .expect_err("expected fund_source parsing to fail")
            .message()
            .to_string()
    }

    #[test]
    fn parses_orchard() {
        let source = FundSource::parse(&json!("orchard"), &mainnet()).unwrap();
        assert!(matches!(source, FundSource::Orchard));
        assert_eq!(source.allowed_pools(), &[ShieldedProtocol::Orchard]);
        // A shielded-only source never permits spending transparent funds.
        assert!(!source.allows_taddr(&taddr(MAINNET_P2PKH)));
    }

    #[test]
    fn parses_sapling() {
        let source = FundSource::parse(&json!("sapling"), &mainnet()).unwrap();
        assert!(matches!(source, FundSource::Sapling));
        assert_eq!(source.allowed_pools(), &[ShieldedProtocol::Sapling]);
        assert!(!source.allows_taddr(&taddr(MAINNET_P2PKH)));
    }

    #[test]
    fn parses_any_transparent() {
        let source = FundSource::parse(&json!("any_transparent"), &mainnet()).unwrap();
        assert!(matches!(source, FundSource::AnyTransparent));
        // No shielded pool is spendable, but any transparent address is.
        assert!(source.allowed_pools().is_empty());
        assert!(source.allows_taddr(&taddr(MAINNET_P2PKH)));
        assert!(source.allows_taddr(&taddr(MAINNET_P2SH)));
    }

    #[test]
    fn parses_transparent_address_array() {
        let source = FundSource::parse(&json!([MAINNET_P2PKH]), &mainnet()).unwrap();
        match &source {
            FundSource::Transparent(set) => {
                assert_eq!(set.len(), 1);
                assert!(set.contains(&taddr(MAINNET_P2PKH)));
            }
            other => panic!("expected Transparent, got {other:?}"),
        }
        assert!(source.allowed_pools().is_empty());
        // Only the listed address is spendable.
        assert!(source.allows_taddr(&taddr(MAINNET_P2PKH)));
        assert!(!source.allows_taddr(&taddr(MAINNET_P2SH)));
    }

    #[test]
    fn parses_multiple_transparent_addresses() {
        let source = FundSource::parse(&json!([MAINNET_P2PKH, MAINNET_P2SH]), &mainnet()).unwrap();
        match &source {
            FundSource::Transparent(set) => {
                assert_eq!(set.len(), 2);
                assert!(set.contains(&taddr(MAINNET_P2PKH)));
                assert!(set.contains(&taddr(MAINNET_P2SH)));
            }
            other => panic!("expected Transparent, got {other:?}"),
        }
    }

    #[test]
    fn deduplicates_repeated_transparent_addresses() {
        let source = FundSource::parse(&json!([MAINNET_P2PKH, MAINNET_P2PKH]), &mainnet()).unwrap();
        match source {
            FundSource::Transparent(set) => assert_eq!(set.len(), 1),
            other => panic!("expected Transparent, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_string() {
        assert_eq!(
            parse_err(json!("transparent")),
            "Invalid fund_source: expected \"orchard\", \"sapling\", \"any_transparent\", or an \
             array of transparent addresses, got \"transparent\".",
        );
    }

    #[test]
    fn rejects_empty_array() {
        assert_eq!(
            parse_err(json!([])),
            "Invalid fund_source: the array of transparent addresses is empty.",
        );
    }

    #[test]
    fn rejects_non_string_array_entry() {
        assert_eq!(
            parse_err(json!([42])),
            "Invalid fund_source: array entries must be transparent address strings.",
        );
    }

    #[test]
    fn rejects_shielded_address_in_array() {
        assert_eq!(
            parse_err(json!([MAINNET_SAPLING])),
            format!("Invalid fund_source: \"{MAINNET_SAPLING}\" is not a transparent address."),
        );
    }

    #[test]
    fn rejects_garbage_address_in_array() {
        assert_eq!(
            parse_err(json!(["not-an-address"])),
            "Invalid fund_source: \"not-an-address\" is not a transparent address.",
        );
    }

    #[test]
    fn rejects_non_string_non_array() {
        let expected = "Invalid fund_source: expected a string or an array of transparent \
                        addresses.";
        assert_eq!(parse_err(json!(42)), expected);
        assert_eq!(parse_err(json!(true)), expected);
        assert_eq!(parse_err(json!({"pool": "orchard"})), expected);
        assert_eq!(parse_err(JsonValue::Null), expected);
    }

    proptest! {
        /// Any string that is not one of the three recognised keywords is reported as an
        /// unknown fund source naming the offending value.
        #[test]
        fn rejects_arbitrary_unknown_keyword(s in "[a-z_]{1,24}") {
            prop_assume!(!matches!(s.as_str(), "orchard" | "sapling" | "any_transparent"));
            let err = FundSource::parse(&json!(s), &mainnet())
                .expect_err("unknown keyword should be rejected");
            let needle = format!("got \"{s}\"");
            prop_assert!(err.message().contains(&needle));
        }

        /// An array of transparent addresses parses into the deduplicated set of those
        /// addresses, regardless of order or repetition.
        #[test]
        fn dedups_transparent_address_array(
            indices in prop::collection::vec(0..2usize, 1..6),
        ) {
            let pool = [MAINNET_P2PKH, MAINNET_P2SH];
            let entries = indices.iter().map(|&i| json!(pool[i])).collect::<Vec<_>>();
            let unique = indices.iter().collect::<HashSet<_>>().len();

            match FundSource::parse(&JsonValue::Array(entries), &mainnet()).unwrap() {
                FundSource::Transparent(set) => {
                    prop_assert_eq!(set.len(), unique);
                    for &i in &indices {
                        prop_assert!(set.contains(&taddr(pool[i])));
                    }
                }
                other => prop_assert!(false, "expected Transparent, got {:?}", other),
            }
        }
    }

    // --- Transparent spend planning (the pure selection/fee/change logic) ---

    /// Builds largest-first P2PKH UTXOs from raw zatoshi values.
    fn p2pkh_utxos(values: &[u64]) -> Vec<(Zatoshis, InputSize)> {
        let mut utxos: Vec<(Zatoshis, InputSize)> = values
            .iter()
            .map(|&v| (Zatoshis::const_from_u64(v), InputSize::STANDARD_P2PKH))
            .collect();
        utxos.sort_by(|a, b| b.0.cmp(&a.0));
        utxos
    }

    /// The ZIP 317 fee for the given count of P2PKH inputs and outputs.
    fn p2pkh_fee(n_inputs: usize, n_outputs: usize) -> u64 {
        let fee = StandardFeeRule::Zip317
            .fee_required(
                &mainnet(),
                BlockHeight::from_u32(0),
                std::iter::repeat_n(InputSize::STANDARD_P2PKH, n_inputs),
                std::iter::repeat_n(P2PKH_STANDARD_OUTPUT_SIZE, n_outputs),
                0,
                0,
                0,
                0,
            )
            .unwrap();
        u64::from(fee)
    }

    /// With no UTXOs, planning fails reporting `have 0` — the unit-level reproduction of the
    /// transparent fund-source bug (where `propose_transfer` selected nothing and reported
    /// `have 0`).
    #[test]
    fn transparent_plan_no_utxos_is_insufficient_with_zero() {
        match plan_transparent_spend(
            &mainnet(),
            &[],
            &[P2PKH_STANDARD_OUTPUT_SIZE],
            Zatoshis::const_from_u64(100_000),
        ) {
            Err(SpendPlanError::Insufficient { have, need }) => {
                assert_eq!(u64::from(have), 0);
                assert!(u64::from(need) >= 100_000);
            }
            _ => panic!("expected Insufficient"),
        }
    }

    /// A single large UTXO funds a small payment and leaves transparent change, exactly.
    #[test]
    fn transparent_plan_emits_change() {
        let utxos = p2pkh_utxos(&[1_000_000]);
        let plan = plan_transparent_spend(
            &mainnet(),
            &utxos,
            &[P2PKH_STANDARD_OUTPUT_SIZE],
            Zatoshis::const_from_u64(200_000),
        )
        .expect("sufficient funds");
        assert_eq!(plan.n_selected, 1);
        assert_eq!(plan.change.len(), 1);
        let change = u64::from(plan.change[0]);
        assert_eq!(1_000_000, 200_000 + change + u64::from(plan.fee));
        assert!(plan.change[0] >= TRANSPARENT_DUST_THRESHOLD);
    }

    /// A remainder below the dust threshold is folded into the fee rather than creating a
    /// change output.
    #[test]
    fn transparent_plan_folds_dust_into_fee() {
        let payment = 100_000u64;
        let fee_no_change = p2pkh_fee(1, 1);
        // Just 1_000 over the change-free requirement; below the 5_000 dust threshold.
        let input = payment + fee_no_change + 1_000;
        let utxos = p2pkh_utxos(&[input]);
        let plan = plan_transparent_spend(
            &mainnet(),
            &utxos,
            &[P2PKH_STANDARD_OUTPUT_SIZE],
            Zatoshis::const_from_u64(payment),
        )
        .expect("sufficient funds");
        assert!(
            plan.change.is_empty(),
            "dust change must be folded into the fee"
        );
        assert_eq!(u64::from(plan.fee), input - payment);
        assert_eq!(input, payment + u64::from(plan.fee));
    }

    /// Selects multiple inputs when no single one covers the payment.
    #[test]
    fn transparent_plan_selects_multiple_inputs() {
        let utxos = p2pkh_utxos(&[60_000, 60_000, 60_000]);
        let plan = plan_transparent_spend(
            &mainnet(),
            &utxos,
            &[P2PKH_STANDARD_OUTPUT_SIZE],
            Zatoshis::const_from_u64(100_000),
        )
        .expect("sufficient funds across multiple inputs");
        assert!(plan.n_selected >= 2);
        let selected: u64 = utxos[..plan.n_selected]
            .iter()
            .map(|(v, _)| u64::from(*v))
            .sum();
        let change: u64 = plan.change.iter().map(|c| u64::from(*c)).sum();
        assert_eq!(selected, 100_000 + change + u64::from(plan.fee));
    }

    proptest! {
        /// For any set of UTXOs and payment total, planning either satisfies the exact balance
        /// equation `Σselected = payments + Σchange + fee` that `Proposal::single_step`
        /// validates, or reports insufficiency consistent with the total available.
        #[test]
        fn transparent_plan_balance_is_exact(
            values in prop::collection::vec(1u64..1_000_000_000u64, 0..12),
            n_payments in 1usize..4,
            payment_total in 1u64..2_000_000_000u64,
        ) {
            let utxos = p2pkh_utxos(&values);
            let payment_sizes = vec![P2PKH_STANDARD_OUTPUT_SIZE; n_payments];
            let total = Zatoshis::const_from_u64(payment_total);

            match plan_transparent_spend(&mainnet(), &utxos, &payment_sizes, total) {
                Ok(plan) => {
                    let selected: u64 =
                        utxos[..plan.n_selected].iter().map(|(v, _)| u64::from(*v)).sum();
                    let change: u64 = plan.change.iter().map(|c| u64::from(*c)).sum();
                    // The invariant `single_step` requires.
                    prop_assert_eq!(selected, payment_total + change + u64::from(plan.fee));
                    prop_assert!(selected >= payment_total + u64::from(plan.fee));
                    prop_assert!(plan.change.len() <= 1);
                    if let Some(c) = plan.change.first() {
                        prop_assert!(*c >= TRANSPARENT_DUST_THRESHOLD);
                    }
                }
                Err(SpendPlanError::Insufficient { have, need }) => {
                    let available: u64 = values.iter().sum();
                    prop_assert_eq!(u64::from(have), available);
                    prop_assert!(u64::from(have) < u64::from(need));
                }
                Err(_) => prop_assert!(false, "unexpected fee/overflow error"),
            }
        }
    }
}
