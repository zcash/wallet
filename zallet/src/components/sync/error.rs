use std::fmt;

use shardtree::error::ShardTreeError;
use zaino_state::FetchServiceError;
use zcash_client_backend::scanning::ScanError;
use zcash_client_sqlite::error::SqliteClientError;

#[derive(Debug)]
pub(crate) enum SyncError {
    Indexer(FetchServiceError),
    Scan(ScanError),
    Tree(ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>),
    Other(SqliteClientError),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Indexer(e) => write!(f, "{e}"),
            SyncError::Scan(e) => write!(f, "{e}"),
            SyncError::Tree(e) => write!(f, "{e}"),
            SyncError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>> for SyncError {
    fn from(e: ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>) -> Self {
        Self::Tree(e)
    }
}

impl From<SqliteClientError> for SyncError {
    fn from(e: SqliteClientError) -> Self {
        Self::Other(e)
    }
}

impl From<FetchServiceError> for SyncError {
    fn from(e: FetchServiceError) -> Self {
        Self::Indexer(e)
    }
}

impl From<zcash_client_backend::data_api::chain::error::Error<SqliteClientError, SyncError>>
    for SyncError
{
    fn from(
        e: zcash_client_backend::data_api::chain::error::Error<SqliteClientError, SyncError>,
    ) -> Self {
        match e {
            zcash_client_backend::data_api::chain::error::Error::Wallet(e) => e.into(),
            zcash_client_backend::data_api::chain::error::Error::BlockSource(e) => e,
            zcash_client_backend::data_api::chain::error::Error::Scan(e) => Self::Scan(e),
        }
    }
}
