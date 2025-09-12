use rand::rngs::OsRng;
use rusqlite::Connection;
use zcash_client_sqlite::{WalletDb, util::SystemClock, wallet::init::WalletMigrator};
use zcash_protocol::consensus::{self, Parameters};

use crate::{components::database, network::Network};

#[cfg(zallet_build = "wallet")]
use crate::components::keystore;

#[test]
fn verify_schema() {
    let mut conn = Connection::open_in_memory().unwrap();
    let mut db_data = WalletDb::from_connection(
        &mut conn,
        Network::Consensus(consensus::Network::MainNetwork),
        SystemClock,
        OsRng,
    );

    WalletMigrator::new()
        .with_external_migrations(database::all_external_migrations(
            db_data.params().network_type(),
        ))
        .init_or_migrate(&mut db_data)
        .unwrap();

    use regex::Regex;
    let re = Regex::new(r"\s+").unwrap();

    let verify_consistency = |query: &str, expected: &[&str]| {
        let mut stmt = conn.prepare(query).unwrap();
        let mut rows = stmt.query([]).unwrap();
        let mut expected_idx = 0;
        while let Some(row) = rows.next().unwrap() {
            let sql: String = row.get(0).unwrap();
            assert_eq!(
                re.replace_all(&sql, " "),
                re.replace_all(expected[expected_idx], " ").trim(),
            );
            expected_idx += 1;
        }
        assert_eq!(expected_idx, expected.len());
    };

    verify_consistency(
        "SELECT sql
        FROM sqlite_schema
        WHERE type = 'table' AND tbl_name LIKE 'ext_zallet_%'
        ORDER BY tbl_name",
        &[
            database::ext::TABLE_VERSION_METADATA,
            database::ext::TABLE_WALLET_METADATA,
            #[cfg(zallet_build = "wallet")]
            keystore::db::TABLE_AGE_RECIPIENTS,
            #[cfg(zallet_build = "wallet")]
            keystore::db::TABLE_LEGACY_SEEDS,
            #[cfg(zallet_build = "wallet")]
            keystore::db::TABLE_MNEMONICS,
            #[cfg(zallet_build = "wallet")]
            keystore::db::TABLE_STANDALONE_SAPLING_KEYS,
            #[cfg(zallet_build = "wallet")]
            keystore::db::TABLE_STANDALONE_TRANSPARENT_KEYS,
        ],
    );

    verify_consistency(
        "SELECT sql
        FROM sqlite_master
        WHERE type = 'index' AND sql != '' AND name LIKE 'ext_zallet_%'
        ORDER BY tbl_name, name",
        &[],
    );

    verify_consistency(
        "SELECT sql
        FROM sqlite_schema
        WHERE type = 'view' AND tbl_name LIKE 'ext_zallet_%'
        ORDER BY tbl_name",
        &[],
    );
}
