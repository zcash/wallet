use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use abscissa_core::Runnable;

use bip0039::{English, Mnemonic};
use secp256k1::PublicKey;
use secrecy::{ExposeSecret, SecretVec};
use shardtree::error::ShardTreeError;
use transparent::address::TransparentAddress;
use zaino_proto::proto::service::TxFilter;
use zaino_state::{FetchServiceError, LightWalletIndexer};
use zcash_client_backend::data_api::{
    Account as _, AccountBirthday, AccountPurpose, AccountSource, WalletRead, WalletWrite as _,
    Zip32Derivation, wallet::decrypt_and_store_transaction,
};
use zcash_client_sqlite::error::SqliteClientError;
use zcash_keys::{
    encoding::AddressCodec,
    keys::{
        DerivationError, UnifiedFullViewingKey,
        zcashd::{PathParseError, ZcashdHdDerivation},
    },
};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::consensus::{BlockHeight, BranchId, NetworkType, Parameters};
use zewif_zcashd::{BDBDump, ZcashdDump, ZcashdParser, ZcashdWallet};
use zip32::{AccountId, fingerprint::SeedFingerprint};

use crate::{
    cli::MigrateZcashdWalletCmd,
    components::{chain_view::ChainView, database::Database, keystore::KeyStore},
    error::{Error, ErrorKind},
    fl,
    prelude::*,
    rosetta::to_chainstate,
};

use super::{AsyncRunnable, migrate_zcash_conf};

/// The ZIP 32 account identifier of the zcashd account used for maintaining legacy `getnewaddress`
/// and `z_getnewaddress` semantics after the zcashd v4.7.0 upgrade to support using
/// mnemonic-sourced HD derivation for all addresses in the wallet.
pub const ZCASHD_LEGACY_ACCOUNT: AccountId = AccountId::const_from_u32(0x7FFFFFFF);
/// A source string to identify an account as being derived from the randomly generated binary HD
/// seed used for Sapling key generation prior to the zcashd v4.7.0 upgrade.
pub const ZCASHD_LEGACY_SOURCE: &str = "zcashd_legacy";
/// A source string to identify an account as being derived from the mnemonic HD seed used for
/// key derivation after the zcashd v4.7.0 upgrade.
pub const ZCASHD_MNEMONIC_SOURCE: &str = "zcashd_mnemonic";

impl AsyncRunnable for MigrateZcashdWalletCmd {
    async fn run(&self) -> Result<(), Error> {
        let config = APP.config();

        if !self.this_is_alpha_code_and_you_will_need_to_redo_the_migration_later {
            return Err(ErrorKind::Generic.context(fl!("migrate-alpha-code")).into());
        }

        // Start monitoring the chain.
        let (chain_view, _chain_indexer_task_handle) = ChainView::new(&config).await?;
        let db = Database::open(&config).await?;
        let keystore = KeyStore::new(&config, db.clone())?;

        let wallet = self.dump_wallet()?;

        Self::migrate_zcashd_wallet(
            db,
            keystore,
            chain_view,
            wallet,
            self.buffer_wallet_transactions,
            self.allow_multiple_wallet_imports,
        )
        .await?;

        Ok(())
    }
}

impl MigrateZcashdWalletCmd {
    fn dump_wallet(&self) -> Result<ZcashdWallet, MigrateError> {
        let wallet_path = if self.path.is_relative() {
            if let Some(datadir) = self.zcashd_datadir.as_ref() {
                datadir.join(&self.path)
            } else {
                migrate_zcash_conf::zcashd_default_data_dir()
                    .ok_or(MigrateError::Wrapped(ErrorKind::Generic.into()))?
                    .join(&self.path)
            }
        } else {
            self.path.to_path_buf()
        };

        let db_dump_path = match &self.zcashd_install_dir {
            Some(path) => {
                let db_dump_path = path.join("zcutil").join("bin").join("db_dump");
                db_dump_path
                    .is_file()
                    .then_some(db_dump_path)
                    .ok_or(which::Error::CannotFindBinaryPath)
            }
            None => which::which("db_dump"),
        };

        if let Ok(db_dump_path) = db_dump_path {
            let db_dump = BDBDump::from_file(db_dump_path.as_path(), wallet_path.as_path())
                .map_err(|e| MigrateError::Zewif {
                    error_type: ZewifError::BdbDump,
                    wallet_path: wallet_path.to_path_buf(),
                    error: e,
                })?;

            let zcashd_dump =
                ZcashdDump::from_bdb_dump(&db_dump, self.allow_warnings).map_err(|e| {
                    MigrateError::Zewif {
                        error_type: ZewifError::ZcashdDump,
                        wallet_path: wallet_path.clone(),
                        error: e,
                    }
                })?;

            let (zcashd_wallet, _unparsed_keys) =
                ZcashdParser::parse_dump(&zcashd_dump, !self.allow_warnings).map_err(|e| {
                    MigrateError::Zewif {
                        error_type: ZewifError::ZcashdDump,
                        wallet_path,
                        error: e,
                    }
                })?;

            Ok(zcashd_wallet)
        } else {
            Err(MigrateError::Wrapped(
                ErrorKind::Generic
                    .context(fl!("err-migrate-wallet-db-dump-not-found"))
                    .into(),
            ))
        }
    }

    async fn chain_tip<C: LightWalletIndexer>(
        chain: &C,
    ) -> Result<Option<BlockHeight>, MigrateError>
    where
        MigrateError: From<C::Error>,
    {
        let tip_height = chain.get_latest_block().await?.height;
        let chain_tip = if tip_height == 0 {
            None
        } else {
            // TODO: this error should go away when we have a better chain data API
            Some(BlockHeight::try_from(tip_height).map_err(|e| {
                ErrorKind::Generic.context(fl!(
                    "err-migrate-wallet-invalid-chain-data",
                    err = e.to_string()
                ))
            })?)
        };

        Ok(chain_tip)
    }

    async fn get_birthday<C: LightWalletIndexer>(
        chain: &C,
        birthday_height: BlockHeight,
        recover_until: Option<BlockHeight>,
    ) -> Result<AccountBirthday, MigrateError>
    where
        MigrateError: From<C::Error>,
    {
        // Fetch the tree state corresponding to the last block prior to the wallet's
        // birthday height.
        let chain_state = to_chainstate(
            chain
                .get_tree_state(zaino_proto::proto::service::BlockId {
                    height: u64::from(birthday_height.saturating_sub(1)),
                    hash: vec![],
                })
                .await?,
        )?;

        Ok(AccountBirthday::from_parts(chain_state, recover_until))
    }

    fn check_network(
        zewif_network: zewif::Network,
        network_type: NetworkType,
    ) -> Result<NetworkType, MigrateError> {
        match (zewif_network, network_type) {
            (zewif::Network::Main, NetworkType::Main) => Ok(()),
            (zewif::Network::Test, NetworkType::Test) => Ok(()),
            (zewif::Network::Regtest, NetworkType::Regtest) => Ok(()),
            (wallet_network, db_network) => Err(MigrateError::NetworkMismatch {
                wallet_network,
                db_network,
            }),
        }?;

        Ok(network_type)
    }

    fn parse_mnemonic(mnemonic: &str) -> Result<Option<Mnemonic>, bip0039::Error> {
        (!mnemonic.is_empty())
            .then(|| Mnemonic::<English>::from_phrase(mnemonic))
            .transpose()
    }

    async fn migrate_zcashd_wallet(
        db: Database,
        keystore: KeyStore,
        chain_view: ChainView,
        wallet: ZcashdWallet,
        buffer_wallet_transactions: bool,
        allow_multiple_wallet_imports: bool,
    ) -> Result<(), MigrateError> {
        let mut db_data = db.handle().await?;
        let network_params = *db_data.params();
        Self::check_network(wallet.network(), network_params.network_type())?;

        let existing_zcash_sourced_accounts = db_data.get_account_ids()?.into_iter().try_fold(
            HashSet::new(),
            |mut found, account_id| {
                let account = db_data
                    .get_account(account_id)?
                    .expect("account exists for just-retrieved id");

                match account.source() {
                    AccountSource::Derived {
                        derivation,
                        key_source,
                    } if key_source.as_ref() == Some(&ZCASHD_MNEMONIC_SOURCE.to_string()) => {
                        found.insert(*derivation.seed_fingerprint());
                    }
                    _ => {}
                }

                Ok::<_, SqliteClientError>(found)
            },
        )?;

        let mnemonic_seed_data = match Self::parse_mnemonic(wallet.bip39_mnemonic().mnemonic())? {
            Some(m) => Some((
                SecretVec::new(m.to_seed("").to_vec()),
                keystore.encrypt_and_store_mnemonic(m).await?,
            )),
            None => None,
        };

        if !existing_zcash_sourced_accounts.is_empty() {
            if allow_multiple_wallet_imports {
                if let Some((seed, _)) = mnemonic_seed_data.as_ref() {
                    let seed_fp =
                        SeedFingerprint::from_seed(seed.expose_secret()).expect("valid length");
                    if existing_zcash_sourced_accounts.contains(&seed_fp) {
                        return Err(MigrateError::DuplicateImport(seed_fp));
                    }
                }
            } else {
                return Err(MigrateError::MultiImportDisabled);
            }
        }

        // Obtain information about the current state of the chain, so that we can set the recovery
        // height properly.
        let chain_subscriber = chain_view.subscribe().await?.inner();
        let chain_tip = Self::chain_tip(&chain_subscriber).await?;
        let sapling_activation = network_params
            .activation_height(zcash_protocol::consensus::NetworkUpgrade::Sapling)
            .expect("Sapling activation height is defined.");

        // Collect an index from txid to block height for all transactions known to the wallet that
        // appear in the main chain.
        let mut tx_heights = HashMap::new();
        for (txid, _) in wallet.transactions().iter() {
            let tx_filter = TxFilter {
                hash: txid.as_ref().to_vec(),
                ..Default::default()
            };
            #[allow(unused_must_use)]
            match chain_subscriber.get_transaction(tx_filter).await {
                Ok(raw_tx) => {
                    let tx_height =
                        BlockHeight::from(u32::try_from(raw_tx.height).map_err(|e| {
                            // TODO: this error should go away when we have a better chain data API
                            ErrorKind::Generic.context(fl!(
                                "err-migrate-wallet-invalid-chain-data",
                                err = e.to_string()
                            ))
                        })?);
                    tx_heights.insert(
                        txid,
                        (tx_height, buffer_wallet_transactions.then_some(raw_tx)),
                    );
                }
                Err(FetchServiceError::TonicStatusError(status))
                    if (status.code() as isize) == (tonic::Code::NotFound as isize) =>
                {
                    // Ignore any transactions that are not in the main chain.
                }
                other => {
                    // FIXME: we should be able to propagate this error, but at present Zaino is
                    // returning all sorts of errors as 500s.
                    dbg!(other);
                }
            }
        }
        info!("Wallet contains {} transactions", tx_heights.len());

        // Since zcashd scans in linear order, we can reliably choose the earliest wallet
        // transaction's mined height as the birthday height, so long as it is in the "stable"
        // range. We don't have a good source of individual per-account birthday information at
        // this point; once we've imported all of the transaction data into the wallet then we'll
        // be able to choose per-account birthdays without difficulty.
        let wallet_birthday = Self::get_birthday(
            &chain_subscriber,
            // Fall back to the chain tip height, and then Sapling activation as a last resort. If
            // we have a birthday height, max() that with sapling activation; that will be the
            // minimum possible wallet birthday that is relevant to future recovery scenarios.
            tx_heights
                .values()
                .map(|(h, _)| h)
                .min()
                .copied()
                .or(chain_tip)
                .map_or(sapling_activation, |h| std::cmp::max(h, sapling_activation)),
            chain_tip,
        )
        .await?;
        info!(
            "Setting the wallet birthday to height {}",
            wallet_birthday.height(),
        );

        let mnemonic_seed_fp = mnemonic_seed_data.as_ref().map(|(_, fp)| *fp);
        let legacy_transparent_account_uuid = if let Some((seed, _)) = mnemonic_seed_data.as_ref() {
            // If there are any legacy transparent keys, create the legacy account.
            if !wallet.keys().is_empty() {
                let (account, _) = db_data.import_account_hd(
                    &format!(
                        "zcashd post-v4.7.0 legacy transparent account {}",
                        u32::from(ZCASHD_LEGACY_ACCOUNT),
                    ),
                    seed,
                    ZCASHD_LEGACY_ACCOUNT,
                    &wallet_birthday,
                    Some(ZCASHD_MNEMONIC_SOURCE),
                )?;

                println!(
                    "{}",
                    fl!(
                        "migrate-wallet-legacy-seed-fp",
                        seed_fp = mnemonic_seed_fp
                            .expect("present for mnemonic seed")
                            .to_string()
                    )
                );

                Some(account.id())
            } else {
                None
            }
        } else {
            None
        };

        let legacy_seed_data = match wallet.legacy_hd_seed() {
            Some(d) => Some((
                SecretVec::new(d.seed_data().to_vec()),
                keystore
                    .encrypt_and_store_legacy_seed(&SecretVec::new(d.seed_data().to_vec()))
                    .await?,
            )),
            None => None,
        };
        let legacy_transparent_account_uuid =
            match (legacy_transparent_account_uuid, legacy_seed_data.as_ref()) {
                (Some(uuid), _) => {
                    // We already had a mnemonic seed and have created the mnemonic-based legacy
                    // account, so we don't need to do anything.
                    Some(uuid)
                }
                (None, Some((seed, _))) if !wallet.keys().is_empty() => {
                    // In this case, we have the legacy seed, but no mnemonic seed was ever derived
                    // from it, so this is a pre-v4.7.0 wallet. We construct the mnemonic in the same
                    // fashion as zcashd, by using the legacy seed as entropy in the generation of the
                    // mnemonic seed, and then import that seed and the associated legacy account so
                    // that we have an account to act as the "bucket of funds" for the transparent keys
                    // derived from system randomness.
                    let mnemonic = zcash_keys::keys::zcashd::derive_mnemonic(seed)
                        .ok_or(ErrorKind::Generic.context(fl!("err-failed-seed-fingerprinting")))?;

                    let seed = SecretVec::new(mnemonic.to_seed("").to_vec());
                    keystore.encrypt_and_store_mnemonic(mnemonic).await?;
                    let (account, _) = db_data.import_account_hd(
                        &format!(
                            "zcashd post-v4.7.0 legacy transparent account {}",
                            u32::from(ZCASHD_LEGACY_ACCOUNT),
                        ),
                        &seed,
                        ZCASHD_LEGACY_ACCOUNT,
                        &wallet_birthday,
                        Some(ZCASHD_MNEMONIC_SOURCE),
                    )?;

                    Some(account.id())
                }
                _ => None,
            };

        let legacy_seed_fp = legacy_seed_data.map(|(_, fp)| fp);

        // Add unified accounts. The only source of unified accounts in zcashd is derivation from
        // the mnemonic seed.
        if wallet.unified_accounts().account_metadata.is_empty() {
            info!("Wallet contains no unified accounts (z_getnewaccount was never used)");
        } else {
            info!(
                "Importing {} unified accounts (created with z_getnewaccount)",
                wallet.unified_accounts().account_metadata.len()
            );
        }
        for (_, account) in wallet.unified_accounts().account_metadata.iter() {
            // The only way that a unified account could be created in zcashd was
            // to be derived from the mnemonic seed, so we can safely unwrap here.
            let (seed, seed_fp) = mnemonic_seed_data
                .as_ref()
                .expect("mnemonic seed should be present");

            assert_eq!(
                SeedFingerprint::from_bytes(*account.seed_fingerprint().as_bytes()),
                *seed_fp
            );

            let zip32_account_id = AccountId::try_from(account.zip32_account_id())
                .map_err(|_| MigrateError::AccountIdInvalid(account.zip32_account_id()))?;

            if db_data
                .get_derived_account(&Zip32Derivation::new(*seed_fp, zip32_account_id, None))?
                .is_none()
            {
                db_data.import_account_hd(
                    &format!(
                        "zcashd imported unified account {}",
                        account.zip32_account_id()
                    ),
                    seed,
                    zip32_account_id,
                    &wallet_birthday,
                    Some(ZCASHD_MNEMONIC_SOURCE),
                )?;
            }
        }

        // Sapling keys may originate from:
        // * The legacy HD seed, under a standard ZIP 32 key path
        // * The mnemonic HD seed, under a standard ZIP 32 key path
        // * The mnemonic HD seed, under the "legacy" account with an additional hardened path element
        // * Zcashd Sapling spending key import
        info!("Importing legacy Sapling keys"); // TODO: Expose how many there are in zewif-zcashd.
        for (idx, key) in wallet.sapling_keys().keypairs().enumerate() {
            // `zewif_zcashd` parses to an earlier version of the `sapling` types, so we
            // must roundtrip through the byte representation into the version we need.
            let extsk = sapling::zip32::ExtendedSpendingKey::from_bytes(&key.extsk().to_bytes())
                .map_err(|_| ()) //work around missing Debug impl
                .expect("Sapling extsk encoding is stable across sapling-crypto versions");
            #[allow(deprecated)]
            let extfvk = extsk.to_extended_full_viewing_key();
            let ufvk =
                UnifiedFullViewingKey::from_sapling_extended_full_viewing_key(extfvk.clone())?;

            let key_seed_fp = key
                .metadata()
                .seed_fp()
                .map(|seed_fp_bytes| SeedFingerprint::from_bytes(*seed_fp_bytes.as_bytes()));

            let derivation = key
                .metadata()
                .hd_keypath()
                .map(|keypath| ZcashdHdDerivation::parse_hd_path(&network_params, keypath))
                .transpose()?
                .zip(key_seed_fp)
                .map(|(derivation, key_seed_fp)| match derivation {
                    ZcashdHdDerivation::Zip32 { account_id } => {
                        Zip32Derivation::new(key_seed_fp, account_id, None)
                    }
                    ZcashdHdDerivation::Post470LegacySapling { address_index } => {
                        Zip32Derivation::new(
                            key_seed_fp,
                            ZCASHD_LEGACY_ACCOUNT,
                            Some(address_index),
                        )
                    }
                });

            // If the key is not associated with either of the seeds, treat it as a standalone
            // imported key
            if key_seed_fp != mnemonic_seed_fp && key_seed_fp != legacy_seed_fp {
                keystore
                    .encrypt_and_store_standalone_sapling_key(&extsk)
                    .await?;
            }

            let account_exists = match key_seed_fp.as_ref() {
                Some(fp) => db_data
                    .get_derived_account(&Zip32Derivation::new(
                        *fp,
                        ZCASHD_LEGACY_ACCOUNT,
                        derivation.as_ref().and_then(|d| d.legacy_address_index()),
                    ))?
                    .is_some(),
                None => db_data.get_account_for_ufvk(&ufvk)?.is_some(),
            };

            if !account_exists {
                db_data.import_account_ufvk(
                    &format!("zcashd legacy sapling {}", idx),
                    &ufvk,
                    &wallet_birthday,
                    AccountPurpose::Spending { derivation },
                    Some(ZCASHD_LEGACY_SOURCE),
                )?;
            }
        }

        // TODO: Move this into zewif-zcashd once we're out of dependency version hell.
        fn convert_key(
            key: &zewif_zcashd::zcashd_wallet::transparent::KeyPair,
        ) -> Result<zcash_keys::keys::transparent::Key, MigrateError> {
            // Check the encoding of the pubkey
            let _ = PublicKey::from_slice(key.pubkey().as_slice())?;
            let compressed = key.pubkey().is_compressed();

            let key = zcash_keys::keys::transparent::Key::der_decode(
                &SecretVec::new(key.privkey().data().to_vec()),
                compressed,
            )
            .map_err(|_| {
                ErrorKind::Generic.context(fl!(
                    "err-migrate-wallet-key-decoding",
                    err = "failed DER decoding"
                ))
            })?;

            Ok(key)
        }

        info!("Importing legacy standalone transparent keys"); // TODO: Expose how many there are in zewif-zcashd.
        for (i, key) in wallet.keys().keypairs().enumerate() {
            let key = convert_key(key)?;
            let pubkey = key.pubkey();
            debug!(
                "[{i}] Importing key for address {}",
                TransparentAddress::from_pubkey(&pubkey).encode(&network_params),
            );

            keystore
                .encrypt_and_store_standalone_transparent_key(&key)
                .await?;

            db_data.import_standalone_transparent_pubkey(
                legacy_transparent_account_uuid.ok_or(MigrateError::SeedNotAvailable)?,
                pubkey,
            )?;
        }

        // Since we've retrieved the raw transaction data anyway, preemptively store it for faster
        // access to balance & to set priorities in the scan queue.
        if buffer_wallet_transactions {
            info!("Importing transactions");
            for (h, raw_tx) in tx_heights.values() {
                let branch_id = BranchId::for_height(&network_params, *h);
                if let Some(raw_tx) = raw_tx {
                    let tx = Transaction::read(&raw_tx.data[..], branch_id)?;
                    db_data.with_mut(|mut db| {
                        decrypt_and_store_transaction(&network_params, &mut db, &tx, Some(*h))
                    })?;
                }
            }
        } else {
            info!("Not importing transactions (--buffer-wallet-transactions not set)");
        }

        Ok(())
    }
}

impl Runnable for MigrateZcashdWalletCmd {
    fn run(&self) {
        self.run_on_runtime();
    }
}

#[derive(Debug)]
pub(crate) enum ZewifError {
    BdbDump,
    ZcashdDump,
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) enum MigrateError {
    Wrapped(Error),
    Zewif {
        error_type: ZewifError,
        wallet_path: PathBuf,
        error: anyhow::Error,
    },
    SeedNotAvailable,
    MnemonicInvalid(bip0039::Error),
    KeyError(secp256k1::Error),
    NetworkMismatch {
        wallet_network: zewif::Network,
        db_network: NetworkType,
    },
    NetworkNotSupported(NetworkType),
    Database(SqliteClientError),
    Tree(ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>),
    Io(std::io::Error),
    Fetch(Box<FetchServiceError>),
    KeyDerivation(DerivationError),
    HdPath(PathParseError),
    AccountIdInvalid(u32),
    MultiImportDisabled,
    DuplicateImport(SeedFingerprint),
}

impl From<MigrateError> for Error {
    fn from(value: MigrateError) -> Self {
        match value {
            MigrateError::Wrapped(e) => e,
            MigrateError::Zewif {
                error_type,
                wallet_path,
                error,
            } => Error::from(match error_type {
                ZewifError::BdbDump => ErrorKind::Generic.context(fl!(
                    "err-migrate-wallet-bdb-parse",
                    path = wallet_path.to_str(),
                    err = error.to_string()
                )),
                ZewifError::ZcashdDump => ErrorKind::Generic.context(fl!(
                    "err-migrate-wallet-db-dump",
                    path = wallet_path.to_str(),
                    err = error.to_string()
                )),
            }),
            MigrateError::SeedNotAvailable => {
                Error::from(ErrorKind::Generic.context(fl!("err-migrate-wallet-seed-absent")))
            }
            MigrateError::MnemonicInvalid(error) => Error::from(ErrorKind::Generic.context(fl!(
                "err-migrate-wallet-invalid-mnemonic",
                err = error.to_string()
            ))),
            MigrateError::KeyError(error) => Error::from(ErrorKind::Generic.context(fl!(
                "err-migrate-wallet-key-decoding",
                err = error.to_string()
            ))),
            MigrateError::NetworkMismatch {
                wallet_network,
                db_network,
            } => Error::from(ErrorKind::Generic.context(fl!(
                "err-migrate-wallet-network-mismatch",
                wallet_network = String::from(wallet_network),
                zallet_network = match db_network {
                    NetworkType::Main => "main",
                    NetworkType::Test => "test",
                    NetworkType::Regtest => "regtest",
                }
            ))),
            MigrateError::NetworkNotSupported(_) => {
                Error::from(ErrorKind::Generic.context(fl!("err-migrate-wallet-regtest")))
            }
            MigrateError::Database(sqlite_client_error) => {
                Error::from(ErrorKind::Generic.context(fl!(
                    "err-migrate-wallet-storage",
                    err = sqlite_client_error.to_string()
                )))
            }
            MigrateError::Tree(e) => Error::from(
                ErrorKind::Generic
                    .context(fl!("err-migrate-wallet-data-parse", err = e.to_string())),
            ),
            MigrateError::Io(e) => Error::from(
                ErrorKind::Generic
                    .context(fl!("err-migrate-wallet-data-parse", err = e.to_string())),
            ),
            MigrateError::Fetch(e) => Error::from(
                ErrorKind::Generic.context(fl!("err-migrate-wallet-tx-fetch", err = e.to_string())),
            ),
            MigrateError::KeyDerivation(e) => Error::from(
                ErrorKind::Generic.context(fl!("err-migrate-wallet-key-data", err = e.to_string())),
            ),
            MigrateError::HdPath(err) => Error::from(ErrorKind::Generic.context(fl!(
                "err-migrate-wallet-data-parse",
                err = format!("{:?}", err)
            ))),
            MigrateError::AccountIdInvalid(id) => Error::from(ErrorKind::Generic.context(fl!(
                "err-migrate-wallet-invalid-account-id",
                account_id = id
            ))),
            MigrateError::MultiImportDisabled => Error::from(
                ErrorKind::Generic.context(fl!("err-migrate-wallet-multi-import-disabled")),
            ),
            MigrateError::DuplicateImport(seed_fingerprint) => {
                Error::from(ErrorKind::Generic.context(fl!(
                    "err-migrate-wallet-duplicate-import",
                    seed_fp = format!("{}", seed_fingerprint)
                )))
            }
        }
    }
}

impl From<ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>> for MigrateError {
    fn from(e: ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>) -> Self {
        Self::Tree(e)
    }
}

impl From<SqliteClientError> for MigrateError {
    fn from(e: SqliteClientError) -> Self {
        Self::Database(e)
    }
}

impl From<bip0039::Error> for MigrateError {
    fn from(value: bip0039::Error) -> Self {
        Self::MnemonicInvalid(value)
    }
}

impl From<Error> for MigrateError {
    fn from(value: Error) -> Self {
        MigrateError::Wrapped(value)
    }
}

impl From<abscissa_core::error::Context<ErrorKind>> for MigrateError {
    fn from(value: abscissa_core::error::Context<ErrorKind>) -> Self {
        MigrateError::Wrapped(value.into())
    }
}

impl From<std::io::Error> for MigrateError {
    fn from(value: std::io::Error) -> Self {
        MigrateError::Io(value)
    }
}

impl From<FetchServiceError> for MigrateError {
    fn from(value: FetchServiceError) -> Self {
        MigrateError::Fetch(Box::new(value))
    }
}

impl From<DerivationError> for MigrateError {
    fn from(value: DerivationError) -> Self {
        MigrateError::KeyDerivation(value)
    }
}

impl From<PathParseError> for MigrateError {
    fn from(value: PathParseError) -> Self {
        MigrateError::HdPath(value)
    }
}

impl From<secp256k1::Error> for MigrateError {
    fn from(value: secp256k1::Error) -> Self {
        MigrateError::KeyError(value)
    }
}
