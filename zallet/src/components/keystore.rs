//! The Zallet keystore.
//!
//! # Design
//!
//! Zallet uses `zcash_client_sqlite` for its wallet, which handles viewing capabilities
//! itself, while leaving key material handling to the application which may have secure
//! storage capabilities (such as provided by mobile platforms). Given that Zallet is a
//! server wallet, we do not assume any secure storage capabilities are available, and
//! instead encrypt key material ourselves.
//!
//! Zallet stores key material (mnemonic seed phrases, standalone spending keys, etc) in
//! the same database as `zcash_client_sqlite`. This simplifies backups (as the wallet
//! operator only has a single database file for both transaction data and key material),
//! and helps to avoid inconsistent state.
//!
//! Zallet uses [`age`] to encrypt key material. age is built around the concept of
//! "encryption recipients" and "decryption identities", and provides several features:
//!
//! - Once the wallet has been initialized for an identity file, key material can be
//!   securely added to the wallet at any time without requiring the identity file.
//! - Key material can be encrypted to multiple recipients, which enables wallet operators
//!   to add redundancy to their backup strategies.
//!   - For example, an operator could configure Zallet with an online identity file used
//!     for regular wallet operations, and an offline identity file used to recover the
//!     key material from the wallet database if the online identity file is lost).
//! - Identity files can themselves be encrypted with a passphrase, allowing the wallet
//!   operator to limit the time for which the age identities are present in memory.
//! - age supports plugins for its encryption and decryption, which enable identities to
//!   be stored on hardware tokens like YubiKeys, or managed by a corporate KMS.
//!
//! ```text
//!  Disk
//! ┌───────────────────────┐       ┌──────────┐
//! │      ┌───────────┐    │       │Passphrase│
//! │      │  File or  │    │       └──────────┘
//! │      │zallet.toml│    │             │
//! │      └───────────┘    │             ▼
//! │            │          │       ┌──────────┐
//! │            ▼          │       │ Decrypt  │
//! │    ┌──────────────┐   │ ┌ ─ ─▶│identities│─ ─ ┐
//! │    │ age identity │   │       └──────────┘
//! │    │     file     │───┼─┘─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─│
//! │    └──────────────┘   │                       │   Memory
//! │                       │             ┌ ─ ─ ─ ─ ┼ ─ ─ ─ ─ ┐
//! │  Database ┌───────────┼─────┐                 ▼
//! │ ┌ ─ ─ ─ ─ ┼ ─ ─ ─ ─ ┐ │     │       │ ┌───────────────┐ │
//! │           ▼           │     └─────────│age identities │──┐
//! │ │ ┌───────────────┐ │ │             │ └───────────────┘ ││
//! │   │age recipients │───┼─────┐                            │
//! │ │ └───────────────┘ │ │     ▼       │    ┌─────────┐    ││  ┌───────────┐
//! │                       │ ┌───────┐        │   Key   │     │  │Transaction│
//! │ │                   │ │ │encrypt│◀┬─┼────│material │────┼┼─▶│  signing  │
//! │     ┌───────────┐     │ └───────┘        └─────────┘     │  └───────────┘
//! │ │   │    age    │   │ │     │     │ │         ▲         ││
//! │     │ciphertext │◀────┼─────┘        ─ ─ ─ ─ ─│─ ─ ─ ─ ─ │
//! │ │   └───────────┘   │ │           │           │          │
//! │  ─ ─ ─ ─ ─│─ ─ ─ ─ ─  │      ┌─────────┐      │          │
//! └───────────┼───────────┘      │Query KMS│      │          │
//!             │                  └─────────┘      │          │
//!             │                       │           │          │
//!             │                               ┌───────┐      │
//!             └───────────────────────┴──────▶│decrypt│◀─────┘
//!                                             └───────┘
//! ```
//!
//! TODO: Integrate or remote thes other notes:
//!
//! - Store recipients in the keystore as common bundles (a la Tink keysets).
//! - Whenever an identity file is directly visible, check it matches the recipients, to
//!   discover incorrect or outdated identity files ASAP.
//!
//! - Encrypt the seed phrase(s) with age, derive any needed keys on-the-fly after
//!   requesting decryption of the relevant seed phrase.
//!   - Could support any or all of the following encryption methods:
//!     - "native identity file" (only plaintext on disk is the age identity, and that
//!       could be on a different disk)
//!     - "passphrase" (like zcashd's experimental wallet encryption)
//!       - The closest analogue to zcashd's experimental wallet encryption would be a
//!         passphrase-encrypted native identity file: need passphrase once to decrypt the
//!         age identity into memory, and then can use the identity to decrypt and access
//!         seed phrases on-the-fly.
//!       - An advantage over the zcashd approach is that you don't need the wallet to be
//!         decrypted in order to import or generate new seed phrases / key material
//!         (zcashd used solely symmetric crypto; native age identities use asymmetric).
//!       - Current downside is that because of the above, encrypted key material would be
//!         quantum-vulnerable (but ML-KEM support is in progress for the age ecosystem).
//!     - "plugin" (enabling key material to be encrypted in a user-specified way e.g. to
//!       a YubiKey, or a corporate KMS)
//!       - Might also want a hybrid approach here to allow for on-first-use decryption
//!         requests rather than every-time decryption requests. Or maybe we want to
//!         support both.
//!   - Zallet would be configured with a corresponding age identity for encrypting /
//!     decrypting seed phrases.
//!   - If the age identity is native and unencrypted, that means Zallet can access seed
//!     phrases whenever it wants. This would be useful in e.g. a Docker deployment, where
//!     the identity could be decrypted during deployment and injected into the correct
//!     location (e.g. via a custom volume).
//!   - If the age identity is passphrase-encrypted, then we could potentially enable the
//!     Bitcoin Core-inherited JSON-RPC methods for decrypting the wallet as the
//!     passphrase UI. The decrypted age identity would be cached in memory until either
//!     an explicit eviction via JSON-RPC or node shutdown.
//!   - If the age identity uses a plugin, then as long as the plugin doesn't require user
//!     interaction the wallet could request decryption on-the-fly during spend actions,
//!     or explicitly via JSON-RPC (with no passphrase).
//!   - If the age identity uses a plugin, and user interaction is required, then we
//!     couldn't support this without Zallet gaining some kind of UI (TUI or GUI) for
//!     users to interact with. Maybe this could be via a dedicated (non-JSON) RPC
//!     protocol between a zallet foobar command and a running zallet start process?
//!     Probably out of scope for the initial impl.

use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use abscissa_core::{component::Injectable, Component, FrameworkError, FrameworkErrorKind};
use abscissa_tokio::TokioComponent;
use bip0039::{English, Mnemonic};
use rusqlite::named_params;
use secrecy::{ExposeSecret, SecretString, SecretVec, Zeroize};
use tokio::sync::RwLock;
use zip32::fingerprint::SeedFingerprint;

use crate::{
    application::ZalletApp,
    config::ZalletConfig,
    error::{Error, ErrorKind},
};

use super::database::Database;

pub(super) mod db;

#[derive(Clone, Default, Injectable)]
#[component(inject = "init_db(zallet::components::database::Database)")]
#[component(inject = "init_tokio(abscissa_tokio::TokioComponent)")]
pub(crate) struct KeyStore {
    db: Option<Database>,

    /// A ciphertext ostensibly containing encrypted age identities, or `None` if the
    /// wallet is not using runtime-encrypted identities.
    encrypted_identities: Option<Vec<u8>>,

    /// The in-memory cache of age identities for decrypting key material.
    identities: Arc<RwLock<Vec<Box<dyn age::Identity + Send + Sync>>>>,
}

impl fmt::Debug for KeyStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyStore").finish_non_exhaustive()
    }
}

impl Component<ZalletApp> for KeyStore {
    fn after_config(&mut self, config: &ZalletConfig) -> Result<(), FrameworkError> {
        // TODO: Maybe support storing the identity in `zallet.toml` instead of as a
        // separate file on disk?
        let path = config.keystore.identity.clone();
        if Path::new(&path).is_relative() {
            return Err(FrameworkErrorKind::ComponentError
                .context(
                    ErrorKind::Init.context("keystore.identity must be an absolute path (for now)"),
                )
                .into());
        }

        // Try parsing as an encrypted age identity.
        let mut identity_data = vec![];
        File::open(&path)?.read_to_end(&mut identity_data)?;
        if let Ok(decryptor) =
            age::Decryptor::new_buffered(age::armor::ArmoredReader::new(identity_data.as_slice()))
        {
            // Only passphrase-encrypted age identities are supported.
            if age::encrypted::EncryptedIdentity::new(decryptor, age::NoCallbacks, None).is_none() {
                return Err(FrameworkErrorKind::ComponentError
                    .context(
                        ErrorKind::Init
                            .context("keystore.identity file is not encrypted with a passphrase"),
                    )
                    .into());
            }

            self.encrypted_identities = Some(identity_data);
        } else {
            // Try parsing as multiple single-line age identities.
            let identity_file =
                age::IdentityFile::from_file(path)?.with_callbacks(age::cli_common::UiCallbacks);
            let identities = identity_file.into_identities().map_err(|e| {
                FrameworkErrorKind::ComponentError.context(
                    ErrorKind::Init.context(format!("keystore.identity file is not usable: {e}")),
                )
            })?;

            *self.identities.blocking_write() = identities;
        }

        Ok(())
    }
}

impl KeyStore {
    /// Called automatically after `Database` is initialized
    pub fn init_db(&mut self, db: &Database) -> Result<(), FrameworkError> {
        self.db = Some(db.clone());
        Ok(())
    }

    /// Called automatically after `TokioComponent` is initialized
    pub fn init_tokio(&mut self, _tokio_cmp: &TokioComponent) -> Result<(), FrameworkError> {
        Ok(())
    }

    /// Returns `true` if the keystore's age identities are runtime-encrypted.
    pub(crate) async fn is_crypted(&self) -> bool {
        self.encrypted_identities.is_some()
    }

    /// Returns `true` if the keystore's age identities are not available for decrypting
    /// key material.
    pub(crate) async fn is_locked(&self) -> bool {
        self.identities.read().await.is_empty()
    }

    async fn with_db<T>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let db = self.db.as_ref().expect("configured");
        db.handle().await?.with_raw(f)
    }

    async fn with_db_mut<T>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let db = self.db.as_ref().expect("configured");
        db.handle().await?.with_raw_mut(f)
    }

    /// Fetches the age recipients for this wallet from the database.
    async fn recipients(&self) -> Result<Vec<Box<dyn age::Recipient + Send>>, Error> {
        self.with_db(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT recipient
                        FROM ext_zallet_keystore_age_recipients",
                )
                .map_err(|e| ErrorKind::Generic.context(e))?;

            let rows = stmt
                .query_map([], |row| row.get(0))
                .map_err(|e| ErrorKind::Generic.context(e))?;
            let recipient_strings = rows
                .collect::<Result<_, _>>()
                .map_err(|e| ErrorKind::Generic.context(e))?;

            // TODO: Replace with a helper with configurable callbacks.
            let mut stdin_guard = age::cli_common::StdinGuard::new(false);
            let recipients = age::cli_common::read_recipients(
                recipient_strings,
                vec![],
                vec![],
                None,
                &mut stdin_guard,
            )
            .map_err(|e| ErrorKind::Generic.context(e))?;

            Ok(recipients)
        })
        .await
    }

    /// Lists the fingerprint of every seed available in the keystore.
    pub(crate) async fn list_seed_fingerprints(&self) -> Result<Vec<SeedFingerprint>, Error> {
        self.with_db(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT hd_seed_fingerprint
                    FROM ext_zallet_keystore_mnemonics",
                )
                .map_err(|e| ErrorKind::Generic.context(e))?;

            let rows = stmt
                .query_map([], |row| row.get(0).map(SeedFingerprint::from_bytes))
                .map_err(|e| ErrorKind::Generic.context(e))?;

            Ok(rows
                .collect::<Result<_, _>>()
                .map_err(|e| ErrorKind::Generic.context(e))?)
        })
        .await
    }

    pub(crate) async fn encrypt_and_store_mnemonic(
        &self,
        mnemonic: SecretString,
    ) -> Result<(), Error> {
        let recipients = self.recipients().await?;

        let mut seed_bytes = Mnemonic::<English>::from_phrase(mnemonic.expose_secret())
            .map_err(|e| ErrorKind::Generic.context(e))?
            .to_seed("");
        seed_bytes.zeroize();
        let seed_fp = SeedFingerprint::from_seed(&seed_bytes).expect("valid length");

        let encrypted_mnemonic = encrypt_string(&recipients, mnemonic.expose_secret())
            .map_err(|e| ErrorKind::Generic.context(e))?;

        self.with_db_mut(|conn| {
            conn.execute(
                "INSERT INTO ext_zallet_keystore_mnemonics
                VALUES (:hd_seed_fingerprint, :encrypted_mnemonic)",
                named_params! {
                    ":hd_seed_fingerprint": seed_fp.to_bytes(),
                    ":encrypted_mnemonic": encrypted_mnemonic,
                },
            )
            .map_err(|e| ErrorKind::Generic.context(e))?;
            Ok(())
        })
        .await?;

        Ok(())
    }

    /// Decrypts the mnemonic phrase corresponding to the given seed fingerprint.
    async fn decrypt_mnemonic(&self, seed_fp: &SeedFingerprint) -> Result<SecretString, Error> {
        // Acquire a read lock on the identities for decryption.
        let identities = self.identities.read().await;
        if identities.is_empty() {
            return Err(ErrorKind::Generic.context("Wallet is locked").into());
        }

        let encrypted_mnemonic = self
            .with_db(|conn| {
                Ok(conn
                    .query_row(
                        "SELECT encrypted_mnemonic
                        FROM ext_zallet_keystore_mnemonics
                        WHERE hd_seed_fingerprint = :hd_seed_fingerprint",
                        named_params! {":hd_seed_fingerprint": seed_fp.to_bytes()},
                        |row| row.get::<_, Vec<u8>>(0),
                    )
                    .map_err(|e| ErrorKind::Generic.context(e))?)
            })
            .await?;

        let mnemonic = decrypt_string(&identities, &encrypted_mnemonic)
            .map_err(|e| ErrorKind::Generic.context(e))?;

        Ok(mnemonic)
    }

    /// Decrypts the seed with the given fingerprint.
    pub(crate) async fn decrypt_seed(
        &self,
        seed_fp: &SeedFingerprint,
    ) -> Result<SecretVec<u8>, Error> {
        let mnemonic = self.decrypt_mnemonic(seed_fp).await?;

        let mut seed_bytes = Mnemonic::<English>::from_phrase(mnemonic.expose_secret())
            .map_err(|e| ErrorKind::Generic.context(e))?
            .to_seed("");
        let seed = SecretVec::new(seed_bytes.to_vec());
        seed_bytes.zeroize();

        Ok(seed)
    }
}

fn encrypt_string(
    recipients: &[Box<dyn age::Recipient + Send>],
    plaintext: &str,
) -> Result<Vec<u8>, age::EncryptError> {
    let encryptor = age::Encryptor::with_recipients(recipients.iter().map(|r| r.as_ref() as _))?;

    let mut ciphertext = Vec::with_capacity(plaintext.len());
    let mut writer = encryptor.wrap_output(&mut ciphertext)?;
    writer.write_all(plaintext.as_bytes())?;
    writer.finish()?;

    Ok(ciphertext)
}

fn decrypt_string(
    identities: &[Box<dyn age::Identity + Send + Sync>],
    ciphertext: &[u8],
) -> Result<SecretString, age::DecryptError> {
    let decryptor = age::Decryptor::new(ciphertext)?;

    // The plaintext is always shorter than the ciphertext. Over-allocating the initial
    // string ensures that no internal re-allocations occur that might leave plaintext
    // bytes strewn around the heap.
    let mut buf = String::with_capacity(ciphertext.len());
    let res = decryptor
        .decrypt(identities.iter().map(|i| i.as_ref() as _))?
        .read_to_string(&mut buf);

    // We intentionally do not use `?` on the decryption expression because doing so in
    // the case of a partial failure could result in part of the secret data being read
    // into `buf`, which would not then be properly zeroized. Instead, we take ownership
    // of the buffer in construction of a `SecretString` to ensure that the memory is
    // zeroed out when we raise the error on the following line.
    let mnemonic = SecretString::new(buf);
    res?;

    Ok(mnemonic)
}
