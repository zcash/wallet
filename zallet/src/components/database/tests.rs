use rand::rngs::OsRng;
use rusqlite::Connection;
use zcash_client_sqlite::{WalletDb, util::SystemClock, wallet::init::WalletMigrator};
use zcash_protocol::consensus;

use crate::{
    components::{database, keystore},
    network::Network,
};

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
        .with_external_migrations(database::all_external_migrations())
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
            keystore::db::TABLE_AGE_RECIPIENTS,
            keystore::db::TABLE_LEGACY_SEEDS,
            keystore::db::TABLE_MNEMONICS,
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
