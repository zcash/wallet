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

use std::collections::{HashMap, HashSet};

use jsonrpsee::core::{JsonValue, RpcResult};
use transparent::{address::TransparentAddress, bundle::OutPoint};
use zcash_client_backend::{
    data_api::{
        AccountMeta, InputSource, NoteFilter, ReceivedNotes, TargetValue, TransparentOutputFilter,
        WalletRead,
        wallet::{ConfirmationsPolicy, TargetHeight},
    },
    wallet::{Note, ReceivedNote, WalletTransparentOutput},
};
use zcash_keys::address::Address;
use zcash_protocol::{ShieldedProtocol, TxId, consensus::BlockHeight};

use crate::{components::database::DbConnection, network::Network};

use super::server::LegacyCode;

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
