#![allow(deprecated)] // For zaino

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    path::PathBuf,
};

use abscissa_core::Runnable;

use bip0039::{English, Mnemonic};
use secp256k1::PublicKey;
use secrecy::{ExposeSecret, SecretVec};
use shardtree::error::ShardTreeError;
use zaino_fetch::jsonrpsee::response::block_header::GetBlockHeader;
use zaino_state::{FetchServiceError, LightWalletIndexer, ZcashIndexer};
use zcash_client_backend::{
    data_api::{
        Account as _, AccountBirthday, AccountPurpose, AccountSource, WalletRead, WalletWrite as _,
        Zip32Derivation, chain::ChainState,
    },
    decrypt_transaction,
};
use zcash_client_sqlite::error::SqliteClientError;
use zcash_keys::keys::{
    DerivationError, UnifiedFullViewingKey,
    zcashd::{PathParseError, ZcashdHdDerivation},
};
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::consensus::{BlockHeight, BranchId, NetworkType, Parameters};
use zcash_script::script::{Code, Redeem};
use zewif_zcashd::{
    BDBDump, ZcashdDump, ZcashdParser, ZcashdWallet,
    parser::prelude::{Parse, Parser},
};
use zip32::{AccountId, fingerprint::SeedFingerprint};

use crate::{
    cli::MigrateZcashdWalletCmd,
    components::{chain::Chain, database::Database, keystore::KeyStore},
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

        // Start monitoring the chain (skip if --no-scan).
        let (chain, _chain_indexer_task_handle) = if self.no_scan {
            (None, None)
        } else {
            let (c, h) = Chain::new(&config).await?;
            (Some(c), Some(h))
        };
        let db = Database::open(&config).await?;
        let keystore = KeyStore::new(&config, db.clone())?;

        info!("Dumping zcashd wallet");
        let (wallet, dump) = self.dump_wallet()?;
        info!("Wallet dumped");

        Self::migrate_zcashd_wallet(
            db,
            keystore,
            chain,
            wallet,
            dump,
            self.buffer_wallet_transactions,
            self.allow_multiple_wallet_imports,
        )
        .await?;

        Ok(())
    }
}

impl MigrateZcashdWalletCmd {
    fn dump_wallet(&self) -> Result<(ZcashdWallet, ZcashdDump), MigrateError> {
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

            Ok((zcashd_wallet, zcashd_dump))
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

    /// Extracts watch-only transparent imports from the raw `zcashd` dump.
    ///
    /// `zcashd` stores `importaddress` / `importpubkey` data in two BDB record types
    /// that `zewif-zcashd` does not currently parse:
    /// * `watchs` records hold every watched `scriptPubKey`. We recover a pubkey from
    ///   each P2PK-form script (added by `importpubkey`); other forms (P2PKH, P2SH,
    ///   etc.) are skipped here because the P2PKH side-effect of `importpubkey` is
    ///   redundant with its P2PK record, and raw-hash watch-only records from
    ///   `importaddress <address>` cannot currently be represented in the Zallet
    ///   wallet schema.
    /// * `cscript` records hold redeem scripts imported via
    ///   `importaddress <redeemScript> "" true`, keyed by their CScriptID. The
    ///   redeem-script bytes are returned directly as parsed `Redeem` scripts.
    fn parse_zcashd_watchonly(
        dump: &ZcashdDump,
    ) -> Result<(Vec<PublicKey>, Vec<Redeem>), MigrateError> {
        // In BDB, the key is `compactsize("watchs") + "watchs" + CScript`, where
        // `CScript` serializes as `compactsize(len) + script_bytes`. `zewif_zcashd`
        // already strips the `"watchs"` prefix for us, leaving the CScript in
        // `DBKey::data`. The record's value is just the byte `'1'` and contains no
        // information we need.
        let mut pubkeys = Vec::new();
        if dump.has_keys_for_keyname("watchs") {
            let records = dump
                .records_for_keyname("watchs")
                .map_err(|e| watchonly_parse_error("watchs", e))?;
            for (key, _value) in records {
                let script_bytes = parse_length_prefixed(key.data.as_ref())
                    .map_err(|e| watchonly_parse_error("watchs script", e))?;
                if let Some(pubkey) = extract_p2pk_pubkey(&script_bytes) {
                    pubkeys.push(pubkey);
                }
                // All other `scriptPubKey` forms are intentionally ignored; see doc
                // comment on `parse_zcashd_watchonly`.
            }
        }

        // For `cscript`, the key is `compactsize("cscript") + "cscript" + 20-byte CScriptID`
        // (the hash is serialized raw, without a length prefix) and the value is the
        // full redeem script (`compactsize(len) + script_bytes`).
        let mut scripts = Vec::new();
        if dump.has_keys_for_keyname("cscript") {
            let records = dump
                .records_for_keyname("cscript")
                .map_err(|e| watchonly_parse_error("cscript", e))?;
            for (_key, value) in records {
                let script_bytes = parse_length_prefixed(value.as_ref())
                    .map_err(|e| watchonly_parse_error("cscript redeem script", e))?;
                match Redeem::parse(&Code(script_bytes)) {
                    Ok(script) => scripts.push(script),
                    Err(e) => tracing::warn!(
                        "Skipping unparseable `zcashd` imported redeem script: {e:?}"
                    ),
                }
            }
        }

        Ok((pubkeys, scripts))
    }

    async fn migrate_zcashd_wallet(
        db: Database,
        keystore: KeyStore,
        chain: Option<Chain>,
        wallet: ZcashdWallet,
        dump: ZcashdDump,
        buffer_wallet_transactions: bool,
        allow_multiple_wallet_imports: bool,
    ) -> Result<(), MigrateError> {
        let mut db_data = db.handle().await?;
        let network_params = *db_data.params();
        Self::check_network(wallet.network(), network_params.network_type())?;

        // Extract scripts imported via `importaddress`/`importpubkey` that `zewif-zcashd` does
        // not surface through `ZcashdWallet`. `watchs` records store watched scriptPubKeys
        // (from `importaddress <address>`, the P2PK/P2PKH scripts added by `importpubkey`, and
        // the P2SH scriptPubKey side-effect of `importaddress <redeemScript> "" true`);
        // `cscript` records store the redeem scripts themselves keyed by CScriptID.
        let (watchonly_pubkeys, watchonly_scripts) = Self::parse_zcashd_watchonly(&dump)?;
        let has_watchonly_imports = !watchonly_pubkeys.is_empty() || !watchonly_scripts.is_empty();

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
        let (chain_subscriber, chain_tip) = if let Some(chain) = &chain {
            let subscriber = chain.subscribe().await?.inner();
            let tip = Self::chain_tip(&subscriber).await?;
            (Some(subscriber), tip)
        } else {
            info!("No-scan mode: skipping chain scanning");
            (None, None)
        };
        let sapling_activation = network_params
            .activation_height(zcash_protocol::consensus::NetworkUpgrade::Sapling)
            .expect("Sapling activation height is defined.");

        // Collect an index from txid to block height for all transactions known to the wallet that
        // appear in the main chain.
        info!(
            "Wallet contains {} transactions",
            wallet.transactions().len(),
        );
        let mut main_chain_block_heights = HashMap::new();
        if let Some(chain_subscriber) = chain_subscriber.as_ref() {
            for (_, wallet_tx) in wallet.transactions().iter() {
                let block_hash = BlockHash(*wallet_tx.hash_block().as_ref());
                // Skip transactions that were unmined when the zcashd wallet was last written.
                if block_hash.0 != [0; 32] {
                    if let Entry::Vacant(entry) = main_chain_block_heights.entry(block_hash) {
                        match chain_subscriber
                            .get_block_header(block_hash.to_string(), true)
                            .await?
                        {
                            GetBlockHeader::Verbose(header) => {
                                entry.insert(BlockHeight::from_u32(header.height));
                            }
                            // Ignore any blocks that are not in the main chain.
                            _ => (),
                        };
                    }
                }
            }
        }
        let mut tx_heights = HashMap::new();
        for (txid, wallet_tx) in wallet.transactions().iter() {
            let block_hash = BlockHash(*wallet_tx.hash_block().as_ref());
            tx_heights.insert(
                txid,
                (
                    main_chain_block_heights.get(&block_hash).cloned(),
                    buffer_wallet_transactions.then_some(wallet_tx),
                ),
            );
        }
        info!(
            "Wallet contains {} mined transactions",
            tx_heights.values().filter(|(h, _)| h.is_some()).count(),
        );

        // Since zcashd scans in linear order, we can reliably choose the earliest wallet
        // transaction's mined height as the birthday height, so long as it is in the "stable"
        // range. We don't have a good source of individual per-account birthday information at
        // this point; once we've imported all of the transaction data into the wallet then we'll
        // be able to choose per-account birthdays without difficulty.
        let wallet_birthday = if let Some(chain_subscriber) = chain_subscriber.as_ref() {
            Self::get_birthday(
                chain_subscriber,
                // Fall back to the chain tip height, and then Sapling activation as a last resort.
                // If we have a birthday height, max() that with sapling activation; that will be
                // the minimum possible wallet birthday that is relevant to future recovery
                // scenarios.
                tx_heights
                    .values()
                    .flat_map(|(h, _)| h)
                    .min()
                    .copied()
                    .or(chain_tip)
                    .map_or(sapling_activation, |h| std::cmp::max(h, sapling_activation)),
                chain_tip,
            )
            .await?
        } else {
            // In no-scan mode, approximate the wallet birthday from transaction expiry
            // heights. Expiry heights are typically creation_height + 40 (the default
            // TX_EXPIRY_DELTA in zcashd). Subtracting 1000 gives a conservative lower
            // bound on the earliest mined height.
            let birthday_height = wallet
                .transactions()
                .values()
                .map(|tx| u32::from(tx.transaction().expiry_height()))
                .filter(|&h| h > 0)
                .min()
                .map(|h| BlockHeight::from_u32(h.saturating_sub(1000)))
                .map(|h| std::cmp::max(h, sapling_activation))
                .unwrap_or(sapling_activation);

            AccountBirthday::from_parts(
                ChainState::empty(birthday_height, BlockHash([0; 32])),
                None,
            )
        };
        info!(
            "Setting the wallet birthday to height {}",
            wallet_birthday.height(),
        );

        let mnemonic_seed_fp = mnemonic_seed_data.as_ref().map(|(_, fp)| *fp);
        let legacy_transparent_account_uuid = if let Some((seed, _)) = mnemonic_seed_data.as_ref() {
            // Create the legacy account if there are any legacy transparent keys or any
            // imported watch-only transparent pubkeys / redeem scripts to store.
            if !wallet.keys().is_empty() || has_watchonly_imports {
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
                (None, Some((seed, _))) if !wallet.keys().is_empty() || has_watchonly_imports => {
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
            if idx % 100 == 0 && idx > 0 {
                info!("Processed {} legacy Sapling keys", idx);
            }
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

        let encryptor = keystore.encryptor().await?;
        let transparent_keypairs = wallet.keys().keypairs().collect::<Vec<_>>();
        info!(
            "Importing {} legacy standalone transparent keys",
            transparent_keypairs.len(),
        ); // TODO: Expose how many there are in zewif-zcashd.
        let encrypted_transparent_keys = transparent_keypairs
            .into_iter()
            .map(|key| {
                let key = convert_key(key)?;
                encryptor.encrypt_standalone_transparent_key(&key)
            })
            .collect::<Result<Vec<_>, _>>()?;
        keystore
            .store_encrypted_standalone_transparent_keys(&encrypted_transparent_keys)
            .await?;
        db_data.import_standalone_transparent_pubkeys(
            legacy_transparent_account_uuid.ok_or(MigrateError::SeedNotAvailable)?,
            encrypted_transparent_keys
                .into_iter()
                .map(|key| *key.pubkey()),
        )?;

        // Import transparent addresses that were added to the `zcashd` wallet via
        // `importaddress` / `importpubkey` (i.e. watch-only imports). These are stored as
        // `watchs` and `cscript` records in `wallet.dat`, and `zewif-zcashd` does not
        // currently surface them through its parsed `ZcashdWallet` type.
        if has_watchonly_imports {
            let target_account =
                legacy_transparent_account_uuid.ok_or(MigrateError::SeedNotAvailable)?;

            info!(
                "Importing {} watch-only transparent pubkey{}",
                watchonly_pubkeys.len(),
                if watchonly_pubkeys.len() == 1 {
                    ""
                } else {
                    "s"
                },
            );
            for pubkey in watchonly_pubkeys {
                db_data.import_standalone_transparent_pubkey(target_account, pubkey)?;
            }

            info!(
                "Importing {} watch-only transparent redeem script{}",
                watchonly_scripts.len(),
                if watchonly_scripts.len() == 1 {
                    ""
                } else {
                    "s"
                },
            );
            for script in watchonly_scripts {
                // `import_standalone_transparent_script` currently only accepts multisig
                // redeem scripts. Log and skip anything else so that a single unsupported
                // script does not abort the migration.
                if let Err(e) = db_data.import_standalone_transparent_script(target_account, script)
                {
                    tracing::warn!(
                        "Skipping unsupported `zcashd` imported P2SH redeem script: {e}"
                    );
                }
            }
        }

        // Since we've retrieved the raw transaction data anyway, preemptively store it for faster
        // access to balance & to set priorities in the scan queue.
        if buffer_wallet_transactions {
            info!("Importing transactions");

            // Fetch the UnifiedFullViewingKeys we are tracking
            let ufvks = db_data.get_unified_full_viewing_keys()?;

            let chain_tip_height = db_data.chain_height()?;

            // Assume that the zcashd wallet was shut down immediately after its last
            // transaction was mined. This will be accurate except in the following case:
            // - User mines a transaction in any older epoch.
            // - A network upgrade activates.
            // - User creates a transaction.
            // - User shuts down the wallet before the transaction is mined.
            let assumed_mempool_height = tx_heights
                .values()
                .flat_map(|(h, _)| h)
                .max()
                .map(|h| *h + 1);

            let mut buf = vec![];
            let decoded_txs = tx_heights
                .values()
                .flat_map(|(h, wallet_tx)| {
                    wallet_tx.map(|wallet_tx| {
                        let consensus_height = match h {
                            Some(h) => *h,
                            None => {
                                let expiry_height =
                                    u32::from(wallet_tx.transaction().expiry_height());
                                if expiry_height == 0 {
                                    // Transaction is unmined and unexpired, use fallback.
                                    assumed_mempool_height.ok_or_else(|| {
                                        ErrorKind::Generic
                                            .context(fl!("err-migrate-wallet-all-unmined"))
                                    })?
                                } else {
                                    // A transaction's expiry height is always in same epoch
                                    // as its eventual mined height.
                                    BlockHeight::from_u32(expiry_height)
                                }
                            }
                        };
                        let consensus_branch_id =
                            BranchId::for_height(&network_params, consensus_height);
                        // TODO: Use the same zcash_primitives version in zewif-zcashd
                        let tx = {
                            buf.clear();
                            wallet_tx.transaction().write(&mut buf)?;
                            Transaction::read(buf.as_slice(), consensus_branch_id)?
                        };
                        Ok((tx, *h))
                    })
                })
                .collect::<Result<Vec<_>, MigrateError>>()?;

            info!("Decrypting {} transactions", decoded_txs.len());
            let decrypted_txs = decoded_txs
                .iter()
                .enumerate()
                .map(|(i, (tx, mined_height))| {
                    if i % 1000 == 0 && i > 0 {
                        tracing::info!("Decrypted {i}/{} transactions", decoded_txs.len());
                    }
                    decrypt_transaction(
                        &network_params,
                        *mined_height,
                        chain_tip_height,
                        tx,
                        &ufvks,
                    )
                })
                .collect::<Vec<_>>();

            info!("Storing {} decrypted transactions", decrypted_txs.len());
            // TODO: Use chunking here if we add support for resuming migrations.
            db_data.store_decrypted_txs(decrypted_txs)?;
        } else {
            info!("Not importing transactions (--buffer-wallet-transactions not set)");
        }

        Ok(())
    }
}

/// Reads a Bitcoin `CompactSize` length prefix followed by that many bytes.
///
/// Used to peel the `CScript` length prefix off `watchs` record keys and
/// `cscript` record values.
fn parse_length_prefixed(bytes: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
    let mut parser = Parser::new(&bytes);
    let data = zewif::Data::parse(&mut parser)?;
    parser.check_finished()?;
    Ok(data.to_vec())
}

/// Returns the pubkey embedded in a `scriptPubKey` of the form
/// `<push33/65> <pubkey> OP_CHECKSIG`, if the script parses as one.
///
/// Returns `None` for any other script shape (P2PKH, P2SH, multisig, etc.).
fn extract_p2pk_pubkey(script: &[u8]) -> Option<PublicKey> {
    const OP_CHECKSIG: u8 = 0xac;
    const PUSH_33: u8 = 0x21;
    const PUSH_65: u8 = 0x41;

    let pubkey_len: usize = match *script.first()? {
        PUSH_33 => 33,
        PUSH_65 => 65,
        _ => return None,
    };
    if script.len() != 2 + pubkey_len || *script.last()? != OP_CHECKSIG {
        return None;
    }
    PublicKey::from_slice(&script[1..1 + pubkey_len]).ok()
}

fn watchonly_parse_error(context: &'static str, err: anyhow::Error) -> MigrateError {
    ErrorKind::Generic
        .context(fl!(
            "err-migrate-wallet-data-parse",
            err = format!("{context}: {err}")
        ))
        .into()
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

#[cfg(test)]
mod tests {
    use super::{extract_p2pk_pubkey, parse_length_prefixed};

    // Compressed P2PK script: OP_PUSHBYTES_33 <33-byte compressed pubkey> OP_CHECKSIG.
    // Pubkey from the `z_importaddress` test vectors.
    const COMPRESSED_P2PK: &[u8] = &[
        0x21, 0x03, 0xb0, 0xda, 0x74, 0x97, 0x30, 0xdc, 0x9b, 0x4b, 0x1f, 0x4a, 0x14, 0xd6, 0x90,
        0x28, 0x77, 0xa9, 0x25, 0x41, 0xf5, 0x36, 0x87, 0x78, 0x85, 0x3d, 0x9c, 0x4a, 0x0c, 0xb7,
        0x80, 0x2d, 0xcf, 0xb2, 0xac,
    ];

    #[test]
    fn extract_p2pk_pubkey_accepts_compressed() {
        let pubkey = extract_p2pk_pubkey(COMPRESSED_P2PK).expect("should parse compressed P2PK");
        assert_eq!(&pubkey.serialize()[..], &COMPRESSED_P2PK[1..34]);
    }

    #[test]
    fn extract_p2pk_pubkey_rejects_p2pkh() {
        // P2PKH: OP_DUP OP_HASH160 OP_PUSHBYTES_20 <hash> OP_EQUALVERIFY OP_CHECKSIG
        let p2pkh = hex::decode("76a91411695b6cd891484c2d49ec5aa738ec2b2f89777788ac").unwrap();
        assert!(extract_p2pk_pubkey(&p2pkh).is_none());
    }

    #[test]
    fn extract_p2pk_pubkey_rejects_p2sh() {
        // P2SH: OP_HASH160 OP_PUSHBYTES_20 <hash> OP_EQUAL
        let p2sh = hex::decode("a91400112233445566778899aabbccddeeff0011223387").unwrap();
        assert!(extract_p2pk_pubkey(&p2sh).is_none());
    }

    #[test]
    fn extract_p2pk_pubkey_rejects_wrong_length() {
        // Correct leading push opcode but the wrong total length.
        let mut bad = COMPRESSED_P2PK.to_vec();
        bad.pop();
        assert!(extract_p2pk_pubkey(&bad).is_none());
    }

    #[test]
    fn extract_p2pk_pubkey_rejects_missing_checksig() {
        let mut bad = COMPRESSED_P2PK.to_vec();
        let last = bad.len() - 1;
        bad[last] = 0x00;
        assert!(extract_p2pk_pubkey(&bad).is_none());
    }

    #[test]
    fn parse_length_prefixed_roundtrips() {
        // `0x03` is a CompactSize(3) prefix for `[0xaa, 0xbb, 0xcc]`.
        let encoded = [0x03, 0xaa, 0xbb, 0xcc];
        assert_eq!(
            parse_length_prefixed(&encoded).unwrap(),
            vec![0xaa, 0xbb, 0xcc]
        );
    }

    #[test]
    fn parse_length_prefixed_rejects_trailing_data() {
        let encoded = [0x01, 0xaa, 0xbb];
        assert!(parse_length_prefixed(&encoded).is_err());
    }
}
