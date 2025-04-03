use abscissa_core::{Component, Runnable, Shutdown};

use crate::{
    application::ZalletApp,
    cli::InitWalletEncryptionCmd,
    components::{database::Database, keystore::KeyStore},
    error::{Error, ErrorKind},
    prelude::*,
};

impl InitWalletEncryptionCmd {
    pub(crate) fn register_components(&self, components: &mut Vec<Box<dyn Component<ZalletApp>>>) {
        // Order these so that dependencies are pushed after the components that use them,
        // to work around a bug: https://github.com/iqlusioninc/abscissa/issues/989
        components.push(Box::new(KeyStore::default()));
        components.push(Box::new(Database::default()));
    }

    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();
        let keystore = APP
            .state()
            .components()
            .get_downcast_ref::<KeyStore>()
            .expect("KeyStore component is registered")
            .clone();

        // TODO: The following logic does not support plugin recipients, which can only be
        // derived from identities by the plugins themselves.

        // If we have encrypted identities, it means the operator configured Zallet with
        // an encrypted identity file; obtain the recipients from it.
        let identity_file = if let Some(identity_file) = keystore
            .decrypt_identity_file(age::cli_common::UiCallbacks)
            .await?
        {
            Ok(identity_file)
        } else {
            // Re-read the identity file from disk.
            age::IdentityFile::from_file(config.keystore.identity.clone())
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
        match abscissa_tokio::run(&APP, self.start()) {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                eprintln!("{}", e);
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
            Err(e) => {
                eprintln!("{}", e);
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
        }
    }
}
