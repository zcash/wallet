//! Test utilities for database operations.

use zcash_client_sqlite::wallet::init::WalletMigrator;
use zcash_protocol::consensus::Parameters;

use super::{DbHandle, all_external_migrations, connection};
use crate::{error::Error, network::Network};

/// Creates an in-memory database pool for testing.
///
/// Uses pool size 2 to allow concurrent access from both the wallet handle
/// and the keystore (which needs its own handle for database operations).
///
/// Uses a shared in-memory database URI so all connections in the pool
/// access the same database.
pub(crate) fn test_pool(params: Network) -> Result<connection::WalletPool, Error> {
    // Use a shared in-memory database with a unique name per pool
    // The `cache=shared` mode allows multiple connections to share the same database
    // The unique name ensures different tests don't interfere with each other
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let db_name = format!(
        "file:zallet_test_{}?mode=memory&cache=shared",
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );

    connection::pool(
        &db_name,
        params,
        Some(connection::PoolConfig { max_size: 2 }),
    )
}

/// A test database wrapper that provides access to the pool and handles.
///
/// This wraps an in-memory SQLite database with all migrations applied,
/// suitable for unit and integration tests.
pub(crate) struct TestDatabase {
    pool: connection::WalletPool,
    params: Network,
}

impl TestDatabase {
    /// Creates a new in-memory test database with migrations applied.
    pub(crate) async fn new(params: Network) -> Result<Self, Error> {
        let pool = test_pool(params)?;
        let db = Self { pool, params };
        db.init().await?;
        Ok(db)
    }

    /// Initializes the database schema by applying all migrations.
    async fn init(&self) -> Result<(), Error> {
        let handle = self.handle().await?;
        let params = self.params;
        handle.with_mut(|mut db_data| {
            WalletMigrator::new()
                .with_external_migrations(all_external_migrations(params.network_type()))
                .init_or_migrate(&mut db_data)
                .map_err(|e| crate::error::ErrorKind::Init.context(e))
        })?;
        Ok(())
    }

    /// Gets a database handle from the pool.
    pub(crate) async fn handle(&self) -> Result<DbHandle, Error> {
        self.pool
            .get()
            .await
            .map_err(|e| crate::error::ErrorKind::Generic.context(e).into())
    }

    /// Returns the network parameters.
    #[allow(dead_code)]
    pub(crate) fn params(&self) -> Network {
        self.params
    }

    /// Returns a reference to the underlying pool.
    ///
    /// This is useful for creating a `Database` wrapper for KeyStore.
    pub(crate) fn pool(&self) -> &connection::WalletPool {
        &self.pool
    }
}
