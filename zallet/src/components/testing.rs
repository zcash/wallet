//! Unified test fixtures for wallet testing.
//!
//! This module provides test utilities for creating in-memory wallets with
//! accounts, suitable for unit and integration testing of wallet functionality.

use bip0039::{Count, English, Mnemonic};
use rand::{RngCore, rngs::OsRng};
use zcash_client_backend::{
    data_api::{AccountBirthday, WalletWrite, chain::ChainState},
    keys::UnifiedSpendingKey,
};
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::BlockHeight;
use zip32::fingerprint::SeedFingerprint;

use crate::{
    components::{
        database::{Database, DbHandle, testing::TestDatabase},
        keystore::{KeyStore, testing::test_keystore},
    },
    error::Error,
    network::Network,
};

/// A complete test wallet environment with database and keystore.
///
/// This provides an in-memory wallet suitable for testing RPC methods
/// and other wallet functionality without requiring disk access.
pub(crate) struct TestWallet {
    db: TestDatabase,
    keystore: KeyStore,
}

impl TestWallet {
    /// Creates a new test wallet with in-memory database.
    pub(crate) async fn new(network: Network) -> Result<Self, Error> {
        let db = TestDatabase::new(network).await?;

        // Create a Database wrapper for KeyStore using the same pool
        let database = Database::from_pool(db.pool().clone());
        let keystore = test_keystore(database).await?;

        Ok(Self { db, keystore })
    }

    /// Gets a database handle.
    pub(crate) async fn handle(&self) -> Result<DbHandle, Error> {
        self.db.handle().await
    }

    /// Returns a reference to the keystore.
    pub(crate) fn keystore(&self) -> &KeyStore {
        &self.keystore
    }

    /// Returns the network parameters.
    #[allow(dead_code)]
    pub(crate) fn params(&self) -> Network {
        self.db.params()
    }

    /// Creates a new test account builder.
    pub(crate) fn account_builder(&self) -> TestAccountBuilder<'_> {
        TestAccountBuilder::new(self)
    }
}

/// Builder for creating test accounts.
pub(crate) struct TestAccountBuilder<'a> {
    wallet: &'a TestWallet,
    mnemonic: Option<Mnemonic<English>>,
    birthday_height: u32,
    name: String,
}

impl<'a> TestAccountBuilder<'a> {
    fn new(wallet: &'a TestWallet) -> Self {
        Self {
            wallet,
            mnemonic: None,
            birthday_height: 1,
            name: "test_account".into(),
        }
    }

    /// Sets a specific mnemonic phrase.
    #[allow(dead_code)]
    pub(crate) fn with_mnemonic(mut self, mnemonic: Mnemonic<English>) -> Self {
        self.mnemonic = Some(mnemonic);
        self
    }

    /// Sets the account birthday height.
    #[allow(dead_code)]
    pub(crate) fn with_birthday(mut self, height: u32) -> Self {
        self.birthday_height = height;
        self
    }

    /// Sets the account name.
    #[allow(dead_code)]
    pub(crate) fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Builds the test account.
    pub(crate) async fn build(self) -> Result<TestAccount, Error> {
        // Generate or use provided mnemonic
        let mnemonic = self.mnemonic.unwrap_or_else(|| {
            // Adapted from `Mnemonic::generate` so we can use `OsRng` directly.
            const BITS_PER_BYTE: usize = 8;
            const MAX_ENTROPY_BITS: usize = Count::Words24.entropy_bits();
            const ENTROPY_BYTES: usize = MAX_ENTROPY_BITS / BITS_PER_BYTE;

            let mut entropy = [0u8; ENTROPY_BYTES];
            OsRng.fill_bytes(&mut entropy);

            Mnemonic::<English>::from_entropy(entropy)
                .expect("valid entropy length won't fail to generate the mnemonic")
        });

        // Store mnemonic in keystore
        let seed_fp = self
            .wallet
            .keystore()
            .encrypt_and_store_mnemonic(mnemonic.clone())
            .await?;

        // Decrypt seed for account creation
        let seed = self.wallet.keystore().decrypt_seed(&seed_fp).await?;

        // Create minimal birthday with empty chain state (no tree data needed for basic tests)
        let chain_state = ChainState::empty(
            BlockHeight::from_u32(self.birthday_height.saturating_sub(1)),
            BlockHash([0; 32]),
        );
        let birthday = AccountBirthday::from_parts(chain_state, None);

        // Create account in database
        let handle = self.wallet.handle().await?;
        let name = self.name.clone();
        let (account_id, usk) = handle
            .with_mut(|mut db| db.create_account(&name, &seed, &birthday, None))
            .map_err(|e| crate::error::ErrorKind::Generic.context(e))?;

        Ok(TestAccount {
            account_id,
            usk,
            seed_fp,
            seed,
            mnemonic,
        })
    }
}

/// A test account with its keys and metadata.
#[allow(dead_code)]
pub(crate) struct TestAccount {
    /// The account identifier in the database.
    pub account_id: zcash_client_sqlite::AccountUuid,
    /// The unified spending key for this account.
    pub usk: UnifiedSpendingKey,
    /// The seed fingerprint.
    pub seed_fp: SeedFingerprint,
    /// The decrypted seed bytes.
    pub seed: secrecy::SecretVec<u8>,
    /// The mnemonic phrase.
    pub mnemonic: Mnemonic<English>,
}
