use abscissa_core::{Runnable, Shutdown};
use bip0039::{English, Mnemonic};
use secrecy::{ExposeSecret, SecretString};

use crate::{
    cli::ImportMnemonicCmd,
    components::{
        database::Database, json_rpc::utils::encode_seedfp_parameter, keystore::KeyStore,
    },
    error::{Error, ErrorKind},
    prelude::*,
};

impl ImportMnemonicCmd {
    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();
        let _lock = config.lock_datadir()?;

        let db = Database::open(&config).await?;
        let keystore = KeyStore::new(&config, db)?;

        let phrase = SecretString::new(
            rpassword::prompt_password("Enter mnemonic:")
                .map_err(|e| ErrorKind::Generic.context(e))?,
        );

        let mnemonic = Mnemonic::<English>::from_phrase(phrase.expose_secret())
            .map_err(|e| ErrorKind::Generic.context(e))?;

        let seedfp = keystore
            .encrypt_and_store_mnemonic(&SecretString::new(mnemonic.into_phrase()))
            .await?;

        println!("Seed fingerprint: {}", encode_seedfp_parameter(&seedfp));

        Ok(())
    }
}

impl Runnable for ImportMnemonicCmd {
    fn run(&self) {
        match abscissa_tokio::run(&APP, self.start()) {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                eprintln!("{e}");
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
            Err(e) => {
                eprintln!("{e}");
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
        }
    }
}
