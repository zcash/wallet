use schemerz_rusqlite::RusqliteMigration;
use zcash_client_sqlite::wallet::init::WalletMigrationError;

mod initial_setup;

pub(in crate::components) fn all() -> Vec<Box<dyn RusqliteMigration<Error = WalletMigrationError>>>
{
    // initial_setup
    vec![Box::new(initial_setup::Migration {})]
}
