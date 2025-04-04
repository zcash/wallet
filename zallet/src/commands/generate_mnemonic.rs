use abscissa_core::{Runnable, Shutdown};
use bip0039::{Count, English, Mnemonic};
use rand::{RngCore, rngs::OsRng};
use secrecy::SecretString;

use crate::{
    cli::GenerateMnemonicCmd,
    components::{database::Database, keystore::KeyStore},
    error::Error,
    prelude::*,
};

impl GenerateMnemonicCmd {
    async fn start(&self) -> Result<(), Error> {
        let config = APP.config();

        let db = Database::open(&config).await?;
        let keystore = KeyStore::new(&config, db)?;

        // Adapted from `Mnemonic::generate` so we can use `OsRng` directly.
        const BITS_PER_BYTE: usize = 8;
        const MAX_ENTROPY_BITS: usize = Count::Words24.entropy_bits();
        const ENTROPY_BYTES: usize = MAX_ENTROPY_BITS / BITS_PER_BYTE;

        let mut entropy = [0u8; ENTROPY_BYTES];
        OsRng.fill_bytes(&mut entropy);

        let mnemonic = Mnemonic::<English>::from_entropy(entropy)
            .expect("valid entropy length won't fail to generate the mnemonic");

        keystore
            .encrypt_and_store_mnemonic(SecretString::new(mnemonic.into_phrase()))
            .await
    }
}

impl Runnable for GenerateMnemonicCmd {
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
