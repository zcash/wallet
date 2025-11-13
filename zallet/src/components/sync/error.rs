use std::fmt;

use shardtree::error::ShardTreeError;
use zcash_client_backend::scanning::ScanError;
use zcash_client_sqlite::error::SqliteClientError;

use crate::error::Error;

#[derive(Debug)]
pub(crate) enum SyncError {
    BatchDecryptorUnavailable,
    Chain(Error),
    Scan(ScanError),
    Tree(Box<ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>>),
    Other(Box<SqliteClientError>),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::BatchDecryptorUnavailable => write!(f, "The batch decryptor has shut down"),
            SyncError::Chain(e) => write!(f, "{e:?}"),
            SyncError::Scan(e) => write!(f, "{e}"),
            SyncError::Tree(e) => write!(f, "{e}"),
            SyncError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>> for SyncError {
    fn from(e: ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>) -> Self {
        Self::Tree(Box::new(e))
    }
}

impl From<SqliteClientError> for SyncError {
    fn from(e: SqliteClientError) -> Self {
        Self::Other(Box::new(e))
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
