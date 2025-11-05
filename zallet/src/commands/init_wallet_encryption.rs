use abscissa_core::Runnable;
use age::secrecy::ExposeSecret;
use std::fs::File;
use std::io::Write;

use crate::{
    cli::InitWalletEncryptionCmd,
    commands::AsyncRunnable,
    components::{database::Database, keystore::KeyStore},
    error::{Error, ErrorKind},
    prelude::*,
};

impl AsyncRunnable for InitWalletEncryptionCmd {
    async fn run(&self) -> Result<(), Error> {
        let config = APP.config();
        let _lock = config.lock_datadir()?;

        let db = Database::open(&config).await?;

        if self.create {
            let path = config.encryption_identity();

            // Q: is this fine or should we make it more like age::file_io::OutputWriter?
            let mut out = File::create_new(&path).map_err(|e| {
                ErrorKind::Init.context(format!("Failed to create encryption identity file: {}", e))
            })?;

            let sk = age::x25519::Identity::generate();
            let pk = sk.to_public();

            // Q: should we also include timestamp?
            writeln!(out, "# public key: {}", pk).map_err(|e| ErrorKind::Init.context(e))?;
            writeln!(out, "{}", sk.to_string().expose_secret())
                .map_err(|e| ErrorKind::Init.context(e))?;

            info!("Encryption identity file created at {}", path.display());
            info!("Public key: {}", pk);
        }

        let keystore = KeyStore::new(&config, db)?;

        // TODO: The following logic does not support plugin recipients, which can only be
        //       derived from identities by the plugins themselves.
        //       https://github.com/zcash/wallet/issues/252

        // If we have encrypted identities, it means the operator configured Zallet with
        // an encrypted identity file; obtain the recipients from it.
        let identity_file = match keystore
            .decrypt_identity_file(age::cli_common::UiCallbacks)
            .await?
        {
            Some(identity_file) => Ok(identity_file),
            _ => {
                // Re-read the identity file from disk.
                age::IdentityFile::from_file(
                    config
                        .encryption_identity()
                        .to_str()
                        .ok_or_else(|| {
                            ErrorKind::Init.context(format!(
                                "{} is not currently supported (not UTF-8)",
                                config.encryption_identity().display(),
                            ))
                        })?
                        .to_string(),
                )
            }
        }
        .map_err(|e| ErrorKind::Generic.context(e))?;

        // Write out a recipients file, then parse it back into recipient strings.
        let mut recipients = vec![];
        identity_file
            .write_recipients_file(&mut recipients)
            .map_err(|e| ErrorKind::Generic.context(e))?;
        let recipient_strings = String::from_utf8(recipients)
            .map_err(|e| ErrorKind::Generic.context(e))?
            .lines()
            .map(String::from)
            .collect();

        // Store the recipients in the keystore.
        keystore.initialize_recipients(recipient_strings).await
    }
}

impl Runnable for InitWalletEncryptionCmd {
    fn run(&self) {
        self.run_on_runtime();
    }
}
