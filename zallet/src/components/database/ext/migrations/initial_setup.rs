use std::collections::HashSet;

use schemerz_rusqlite::RusqliteMigration;
use uuid::Uuid;
use zcash_client_sqlite::wallet::init::{WalletMigrationError, migrations::V_0_15_0};

pub(super) const MIGRATION_ID: Uuid = Uuid::from_u128(0xa2b3f7ed_b2ec_4b92_a390_3f9bed3f0324);

pub(super) struct Migration;

impl schemerz::Migration<Uuid> for Migration {
    fn id(&self) -> Uuid {
        MIGRATION_ID
    }

    fn dependencies(&self) -> HashSet<Uuid> {
        V_0_15_0.iter().copied().collect()
    }

    fn description(&self) -> &'static str {
        "Initializes the Zallet top-level extension tables."
    }
}

impl RusqliteMigration for Migration {
    type Error = WalletMigrationError;

    fn up(&self, transaction: &rusqlite::Transaction<'_>) -> Result<(), Self::Error> {
        transaction.execute_batch(
            "CREATE TABLE ext_zallet_db_version_metadata (
                version STRING NOT NULL,
                git_revision STRING,
                clean INTEGER,
                migrated TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    fn down(&self, _transaction: &rusqlite::Transaction<'_>) -> Result<(), Self::Error> {
        Ok(())
    }
}
