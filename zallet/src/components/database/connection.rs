use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, RwLock};

use rand::rngs::OsRng;
use secrecy::SecretVec;
use shardtree::{ShardTree, error::ShardTreeError};
use transparent::{address::TransparentAddress, bundle::OutPoint, keys::NonHardenedChildIndex};
use zcash_client_backend::data_api::wallet::{ConfirmationsPolicy, TargetHeight};
use zcash_client_backend::data_api::{AddressInfo, Balance, TargetValue, Zip32Derivation};
use zcash_client_backend::{
    address::UnifiedAddress,
    data_api::{
        AccountBirthday, AccountMeta, InputSource, NoteFilter, ORCHARD_SHARD_HEIGHT,
        SAPLING_SHARD_HEIGHT, SpendableNotes, WalletCommitmentTrees, WalletRead, WalletWrite,
    },
    keys::{UnifiedAddressRequest, UnifiedFullViewingKey, UnifiedSpendingKey},
    wallet::{Note, ReceivedNote, TransparentAddressMetadata, WalletTransparentOutput},
};
use zcash_client_sqlite::{WalletDb, util::SystemClock};
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::{ShieldedProtocol, consensus::BlockHeight};
use zip32::DiversifierIndex;

use crate::{
    error::{Error, ErrorKind},
    network::Network,
};

pub(super) fn pool(path: impl AsRef<Path>, params: Network) -> Result<WalletPool, Error> {
    let config = deadpool_sqlite::Config::new(path.as_ref());
    let manager = WalletManager::from_config(&config, params);
    WalletPool::builder(manager)
        .config(deadpool::managed::PoolConfig::default())
        .build()
        .map_err(|e| ErrorKind::Generic.context(e).into())
}

pub(super) type WalletPool = deadpool::managed::Pool<WalletManager>;

pub(crate) struct WalletManager {
    inner: deadpool_sqlite::Manager,
    /// Connection pools are thread-safe, but SQLite does not reliably follow the busy
    /// handler (configured by `rusqlite` to a timeout after 5s), so we explicitly guard
    /// against SQLite `DatabaseBusy` errors.
    lock: Arc<RwLock<()>>,
    params: Network,
}

impl WalletManager {
    /// Creates a new [`WalletManager`] using the given [`deadpool_sqlite::Config`] backed
    /// by the specified [`deadpool_sqlite::Runtime`].
    #[must_use]
    pub fn from_config(config: &deadpool_sqlite::Config, params: Network) -> Self {
        Self {
            inner: deadpool_sqlite::Manager::from_config(config, deadpool_sqlite::Runtime::Tokio1),
            lock: Arc::new(RwLock::new(())),
            params,
        }
    }
}

impl deadpool::managed::Manager for WalletManager {
    type Type = DbConnection;
    type Error = rusqlite::Error;

    async fn create(&self) -> Result<Self::Type, Self::Error> {
        let inner = self.inner.create().await?;
        inner
            .interact(|conn| rusqlite::vtab::array::load_module(conn))
            .await
            .map_err(|_| rusqlite::Error::UnwindingPanic)??;
        Ok(DbConnection {
            inner,
            lock: self.lock.clone(),
            params: self.params,
        })
    }

    async fn recycle(
        &self,
        obj: &mut Self::Type,
        metrics: &deadpool_sqlite::Metrics,
    ) -> deadpool::managed::RecycleResult<Self::Error> {
        self.inner.recycle(&mut obj.inner, metrics).await
    }
}

pub(crate) struct DbConnection {
    inner: deadpool_sync::SyncWrapper<rusqlite::Connection>,
    lock: Arc<RwLock<()>>,
    params: Network,
}

impl DbConnection {
    pub(crate) fn params(&self) -> &Network {
        &self.params
    }

    fn with<T>(
        &self,
        f: impl FnOnce(WalletDb<&rusqlite::Connection, Network, SystemClock, OsRng>) -> T,
    ) -> T {
        tokio::task::block_in_place(|| {
            let _guard = self.lock.read().unwrap();
            f(WalletDb::from_connection(
                self.inner.lock().unwrap().as_ref(),
                self.params,
                SystemClock,
                OsRng,
            ))
        })
    }

    pub(crate) fn with_mut<T>(
        &self,
        f: impl FnOnce(WalletDb<&mut rusqlite::Connection, Network, SystemClock, OsRng>) -> T,
    ) -> T {
        tokio::task::block_in_place(|| {
            let _guard = self.lock.write().unwrap();
            f(WalletDb::from_connection(
                self.inner.lock().unwrap().as_mut(),
                self.params,
                SystemClock,
                OsRng,
            ))
        })
    }

    pub(crate) fn with_raw<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
        tokio::task::block_in_place(|| {
            let _guard = self.lock.read().unwrap();
            f(self.inner.lock().unwrap().as_ref())
        })
    }

    pub(crate) fn with_raw_mut<T>(&self, f: impl FnOnce(&mut rusqlite::Connection) -> T) -> T {
        tokio::task::block_in_place(|| {
            let _guard = self.lock.write().unwrap();
            f(self.inner.lock().unwrap().as_mut())
        })
    }
}

impl WalletRead for DbConnection {
    type Error = <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletRead>::Error;
    type AccountId =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletRead>::AccountId;
    type Account =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletRead>::Account;

    fn get_account_ids(&self) -> Result<Vec<Self::AccountId>, Self::Error> {
        self.with(|db_data| db_data.get_account_ids())
    }

    fn get_account(
        &self,
        account_id: Self::AccountId,
    ) -> Result<Option<Self::Account>, Self::Error> {
        self.with(|db_data| db_data.get_account(account_id))
    }

    fn get_derived_account(
        &self,
        derivation: &Zip32Derivation,
    ) -> Result<Option<Self::Account>, Self::Error> {
        self.with(|db_data| db_data.get_derived_account(derivation))
    }

    fn validate_seed(
        &self,
        account_id: Self::AccountId,
        seed: &SecretVec<u8>,
    ) -> Result<bool, Self::Error> {
        self.with(|db_data| db_data.validate_seed(account_id, seed))
    }

    fn seed_relevance_to_derived_accounts(
        &self,
        seed: &SecretVec<u8>,
    ) -> Result<zcash_client_backend::data_api::SeedRelevance<Self::AccountId>, Self::Error> {
        self.with(|db_data| db_data.seed_relevance_to_derived_accounts(seed))
    }

    fn get_account_for_ufvk(
        &self,
        ufvk: &UnifiedFullViewingKey,
    ) -> Result<Option<Self::Account>, Self::Error> {
        self.with(|db_data| db_data.get_account_for_ufvk(ufvk))
    }

    fn list_addresses(&self, account: Self::AccountId) -> Result<Vec<AddressInfo>, Self::Error> {
        self.with(|db_data| db_data.list_addresses(account))
    }

    fn get_last_generated_address_matching(
        &self,
        account: Self::AccountId,
        address_filter: UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        self.with(|db_data| db_data.get_last_generated_address_matching(account, address_filter))
    }

    fn get_account_birthday(&self, account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        self.with(|db_data| db_data.get_account_birthday(account))
    }

    fn get_wallet_birthday(&self) -> Result<Option<BlockHeight>, Self::Error> {
        self.with(|db_data| db_data.get_wallet_birthday())
    }

    fn get_wallet_summary(
        &self,
        confirmations_policy: ConfirmationsPolicy,
    ) -> Result<Option<zcash_client_backend::data_api::WalletSummary<Self::AccountId>>, Self::Error>
    {
        self.with(|db_data| db_data.get_wallet_summary(confirmations_policy))
    }

    fn chain_height(&self) -> Result<Option<BlockHeight>, Self::Error> {
        self.with(|db_data| db_data.chain_height())
    }

    fn get_block_hash(&self, block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        self.with(|db_data| db_data.get_block_hash(block_height))
    }

    fn block_metadata(
        &self,
        height: BlockHeight,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        self.with(|db_data| db_data.block_metadata(height))
    }

    fn block_fully_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        self.with(|db_data| db_data.block_fully_scanned())
    }

    fn get_max_height_hash(&self) -> Result<Option<(BlockHeight, BlockHash)>, Self::Error> {
        self.with(|db_data| db_data.get_max_height_hash())
    }

    fn block_max_scanned(
        &self,
    ) -> Result<Option<zcash_client_backend::data_api::BlockMetadata>, Self::Error> {
        self.with(|db_data| db_data.block_max_scanned())
    }

    fn suggest_scan_ranges(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::scanning::ScanRange>, Self::Error> {
        self.with(|db_data| db_data.suggest_scan_ranges())
    }

    fn get_target_and_anchor_heights(
        &self,
        min_confirmations: std::num::NonZeroU32,
    ) -> Result<Option<(TargetHeight, BlockHeight)>, Self::Error> {
        self.with(|db_data| db_data.get_target_and_anchor_heights(min_confirmations))
    }

    fn get_tx_height(
        &self,
        txid: zcash_protocol::TxId,
    ) -> Result<Option<BlockHeight>, Self::Error> {
        self.with(|db_data| db_data.get_tx_height(txid))
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<Self::AccountId, UnifiedFullViewingKey>, Self::Error> {
        self.with(|db_data| db_data.get_unified_full_viewing_keys())
    }

    fn get_memo(
        &self,
        note_id: zcash_client_backend::wallet::NoteId,
    ) -> Result<Option<zcash_protocol::memo::Memo>, Self::Error> {
        self.with(|db_data| db_data.get_memo(note_id))
    }

    fn get_transaction(
        &self,
        txid: zcash_protocol::TxId,
    ) -> Result<Option<Transaction>, Self::Error> {
        self.with(|db_data| db_data.get_transaction(txid))
    }

    fn get_sapling_nullifiers(
        &self,
        query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, sapling::Nullifier)>, Self::Error> {
        self.with(|db_data| db_data.get_sapling_nullifiers(query))
    }

    fn get_orchard_nullifiers(
        &self,
        query: zcash_client_backend::data_api::NullifierQuery,
    ) -> Result<Vec<(Self::AccountId, orchard::note::Nullifier)>, Self::Error> {
        self.with(|db_data| db_data.get_orchard_nullifiers(query))
    }

    fn get_transparent_receivers(
        &self,
        account: Self::AccountId,
        include_change: bool,
    ) -> Result<HashMap<TransparentAddress, Option<TransparentAddressMetadata>>, Self::Error> {
        self.with(|db_data| db_data.get_transparent_receivers(account, include_change))
    }

    fn get_transparent_balances(
        &self,
        account: Self::AccountId,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
    ) -> Result<HashMap<TransparentAddress, Balance>, Self::Error> {
        self.with(|db_data| {
            db_data.get_transparent_balances(account, target_height, confirmations_policy)
        })
    }

    fn get_transparent_address_metadata(
        &self,
        account: Self::AccountId,
        address: &TransparentAddress,
    ) -> Result<Option<TransparentAddressMetadata>, Self::Error> {
        self.with(|db_data| db_data.get_transparent_address_metadata(account, address))
    }

    fn utxo_query_height(&self, account: Self::AccountId) -> Result<BlockHeight, Self::Error> {
        self.with(|db_data| db_data.utxo_query_height(account))
    }

    fn get_known_ephemeral_addresses(
        &self,
        account: Self::AccountId,
        index_range: Option<Range<NonHardenedChildIndex>>,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, Self::Error> {
        self.with(|db_data| db_data.get_known_ephemeral_addresses(account, index_range))
    }

    fn find_account_for_ephemeral_address(
        &self,
        address: &TransparentAddress,
    ) -> Result<Option<Self::AccountId>, Self::Error> {
        self.with(|db_data| db_data.find_account_for_ephemeral_address(address))
    }

    fn transaction_data_requests(
        &self,
    ) -> Result<Vec<zcash_client_backend::data_api::TransactionDataRequest>, Self::Error> {
        self.with(|db_data| db_data.transaction_data_requests())
    }
}

impl InputSource for DbConnection {
    type Error =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as InputSource>::Error;
    type AccountId =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as InputSource>::AccountId;
    type NoteRef =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as InputSource>::NoteRef;

    fn get_spendable_note(
        &self,
        txid: &zcash_protocol::TxId,
        protocol: ShieldedProtocol,
        index: u32,
    ) -> Result<Option<ReceivedNote<Self::NoteRef, Note>>, Self::Error> {
        self.with(|db_data| db_data.get_spendable_note(txid, protocol, index))
    }

    fn select_spendable_notes(
        &self,
        account: Self::AccountId,
        target_value: TargetValue,
        sources: &[ShieldedProtocol],
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
        exclude: &[Self::NoteRef],
    ) -> Result<SpendableNotes<Self::NoteRef>, Self::Error> {
        self.with(|db_data| {
            db_data.select_spendable_notes(
                account,
                target_value,
                sources,
                target_height,
                confirmations_policy,
                exclude,
            )
        })
    }

    fn get_unspent_transparent_output(
        &self,
        outpoint: &OutPoint,
    ) -> Result<Option<WalletTransparentOutput>, Self::Error> {
        self.with(|db_data| db_data.get_unspent_transparent_output(outpoint))
    }

    fn get_spendable_transparent_outputs(
        &self,
        address: &TransparentAddress,
        target_height: TargetHeight,
        confirmations_policy: ConfirmationsPolicy,
    ) -> Result<Vec<WalletTransparentOutput>, Self::Error> {
        self.with(|db_data| {
            db_data.get_spendable_transparent_outputs(address, target_height, confirmations_policy)
        })
    }

    fn get_account_metadata(
        &self,
        account: Self::AccountId,
        selector: &NoteFilter,
        exclude: &[Self::NoteRef],
    ) -> Result<AccountMeta, Self::Error> {
        self.with(|db_data| db_data.get_account_metadata(account, selector, exclude))
    }
}

impl WalletWrite for DbConnection {
    type UtxoRef =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletWrite>::UtxoRef;

    fn create_account(
        &mut self,
        account_name: &str,
        seed: &SecretVec<u8>,
        birthday: &AccountBirthday,
        key_source: Option<&str>,
    ) -> Result<(Self::AccountId, UnifiedSpendingKey), Self::Error> {
        self.with_mut(|mut db_data| {
            db_data.create_account(account_name, seed, birthday, key_source)
        })
    }

    fn import_account_hd(
        &mut self,
        account_name: &str,
        seed: &SecretVec<u8>,
        account_index: zip32::AccountId,
        birthday: &AccountBirthday,
        key_source: Option<&str>,
    ) -> Result<(Self::Account, UnifiedSpendingKey), Self::Error> {
        self.with_mut(|mut db_data| {
            db_data.import_account_hd(account_name, seed, account_index, birthday, key_source)
        })
    }

    fn import_account_ufvk(
        &mut self,
        account_name: &str,
        unified_key: &UnifiedFullViewingKey,
        birthday: &AccountBirthday,
        purpose: zcash_client_backend::data_api::AccountPurpose,
        key_source: Option<&str>,
    ) -> Result<Self::Account, Self::Error> {
        self.with_mut(|mut db_data| {
            db_data.import_account_ufvk(account_name, unified_key, birthday, purpose, key_source)
        })
    }

    fn get_next_available_address(
        &mut self,
        account: Self::AccountId,
        request: UnifiedAddressRequest,
    ) -> Result<Option<(UnifiedAddress, DiversifierIndex)>, Self::Error> {
        self.with_mut(|mut db_data| db_data.get_next_available_address(account, request))
    }

    fn get_address_for_index(
        &mut self,
        account: Self::AccountId,
        diversifier_index: DiversifierIndex,
        request: UnifiedAddressRequest,
    ) -> Result<Option<UnifiedAddress>, Self::Error> {
        self.with_mut(|mut db_data| {
            db_data.get_address_for_index(account, diversifier_index, request)
        })
    }

    fn update_chain_tip(&mut self, tip_height: BlockHeight) -> Result<(), Self::Error> {
        self.with_mut(|mut db_data| db_data.update_chain_tip(tip_height))
    }

    fn put_blocks(
        &mut self,
        from_state: &zcash_client_backend::data_api::chain::ChainState,
        blocks: Vec<zcash_client_backend::data_api::ScannedBlock<Self::AccountId>>,
    ) -> Result<(), Self::Error> {
        self.with_mut(|mut db_data| db_data.put_blocks(from_state, blocks))
    }

    fn put_received_transparent_utxo(
        &mut self,
        output: &WalletTransparentOutput,
    ) -> Result<Self::UtxoRef, Self::Error> {
        self.with_mut(|mut db_data| db_data.put_received_transparent_utxo(output))
    }

    fn store_decrypted_tx(
        &mut self,
        received_tx: zcash_client_backend::data_api::DecryptedTransaction<'_, Self::AccountId>,
    ) -> Result<(), Self::Error> {
        self.with_mut(|mut db_data| db_data.store_decrypted_tx(received_tx))
    }

    fn store_transactions_to_be_sent(
        &mut self,
        transactions: &[zcash_client_backend::data_api::SentTransaction<'_, Self::AccountId>],
    ) -> Result<(), Self::Error> {
        self.with_mut(|mut db_data| db_data.store_transactions_to_be_sent(transactions))
    }

    fn truncate_to_height(&mut self, max_height: BlockHeight) -> Result<BlockHeight, Self::Error> {
        self.with_mut(|mut db_data| db_data.truncate_to_height(max_height))
    }

    fn reserve_next_n_ephemeral_addresses(
        &mut self,
        account_id: Self::AccountId,
        n: usize,
    ) -> Result<Vec<(TransparentAddress, TransparentAddressMetadata)>, Self::Error> {
        self.with_mut(|mut db_data| db_data.reserve_next_n_ephemeral_addresses(account_id, n))
    }

    fn set_transaction_status(
        &mut self,
        txid: zcash_protocol::TxId,
        status: zcash_client_backend::data_api::TransactionStatus,
    ) -> Result<(), Self::Error> {
        self.with_mut(|mut db_data| db_data.set_transaction_status(txid, status))
    }

    fn notify_address_checked(
        &mut self,
        request: zcash_client_backend::data_api::TransactionsInvolvingAddress,
        as_of_height: BlockHeight,
    ) -> Result<(), Self::Error> {
        self.with_mut(|mut db_data| db_data.notify_address_checked(request, as_of_height))
    }
}

impl WalletCommitmentTrees for DbConnection {
    type Error =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletCommitmentTrees>::Error;
    type SaplingShardStore<'a> =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletCommitmentTrees>::SaplingShardStore<'a>;

    fn with_sapling_tree_mut<F, A, E>(&mut self, callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::SaplingShardStore<'a>,
                { sapling::NOTE_COMMITMENT_TREE_DEPTH },
                SAPLING_SHARD_HEIGHT,
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        self.with_mut(|mut db_data| db_data.with_sapling_tree_mut(callback))
    }

    fn put_sapling_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[zcash_client_backend::data_api::chain::CommitmentTreeRoot<sapling::Node>],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_mut(|mut db_data| db_data.put_sapling_subtree_roots(start_index, roots))
    }

    type OrchardShardStore<'a> =
        <WalletDb<rusqlite::Connection, Network, SystemClock, OsRng> as WalletCommitmentTrees>::OrchardShardStore<'a>;

    fn with_orchard_tree_mut<F, A, E>(&mut self, callback: F) -> Result<A, E>
    where
        for<'a> F: FnMut(
            &'a mut ShardTree<
                Self::OrchardShardStore<'a>,
                { ORCHARD_SHARD_HEIGHT * 2 },
                ORCHARD_SHARD_HEIGHT,
            >,
        ) -> Result<A, E>,
        E: From<ShardTreeError<Self::Error>>,
    {
        self.with_mut(|mut db_data| db_data.with_orchard_tree_mut(callback))
    }

    fn put_orchard_subtree_roots(
        &mut self,
        start_index: u64,
        roots: &[zcash_client_backend::data_api::chain::CommitmentTreeRoot<
            orchard::tree::MerkleHashOrchard,
        >],
    ) -> Result<(), ShardTreeError<Self::Error>> {
        self.with_mut(|mut db_data| db_data.put_orchard_subtree_roots(start_index, roots))
    }
}
