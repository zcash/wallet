use abscissa_core::{Component, Runnable, Shutdown};
use bip0039::{Count, English, Mnemonic};
use rand::{rngs::OsRng, RngCore};
use secrecy::SecretString;

use crate::{
    application::ZalletApp,
    cli::GenerateMnemonicCmd,
    components::{database::Database, keystore::KeyStore},
    error::Error,
    prelude::*,
};

impl GenerateMnemonicCmd {
    pub(crate) fn register_components(&self, components: &mut Vec<Box<dyn Component<ZalletApp>>>) {
        // Order these so that dependencies are pushed after the components that use them,
        // to work around a bug: https://github.com/iqlusioninc/abscissa/issues/989
        components.push(Box::new(KeyStore::default()));
        components.push(Box::new(Database::default()));
    }

    async fn start(&self) -> Result<(), Error> {
        let keystore = APP
            .state()
            .components()
            .get_downcast_ref::<KeyStore>()
            .expect("KeyStore component is registered")
            .clone();

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
