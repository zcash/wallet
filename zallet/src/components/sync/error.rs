use std::fmt;

use shardtree::error::ShardTreeError;
use zaino_fetch::jsonrpc::error::JsonRpcConnectorError;
use zaino_state::error::{BlockCacheError, MempoolError, StatusError};
use zcash_client_backend::scanning::ScanError;
use zcash_client_sqlite::error::SqliteClientError;

#[derive(Debug)]
pub(crate) enum SyncError {
    BlockCache(BlockCacheError),
    Indexer(StatusError),
    Mempool(MempoolError),
    Node(JsonRpcConnectorError),
    Scan(ScanError),
    Tree(ShardTreeError<zcash_client_sqlite::wallet::commitment_tree::Error>),
    Other(SqliteClientError),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::BlockCache(e) => write!(f, "{e}"),
            SyncError::Indexer(e) => write!(f, "{e}"),
            SyncError::Mempool(e) => write!(f, "{e}"),
            SyncError::Node(e) => write!(f, "{e}"),
            SyncError::Scan(e) => write!(f, "{e}"),
            SyncError::Tree(e) => write!(f, "{e}"),
            SyncError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<BlockCacheError> for SyncError {
    fn from(e: BlockCacheError) -> Self {
        Self::BlockCache(e)
    }
}

impl From<JsonRpcConnectorError> for SyncError {
    fn from(e: JsonRpcConnectorError) -> Self {
        Self::Node(e)
    }
}

impl From<MempoolError> for SyncError {
    fn from(e: MempoolError) -> Self {
        Self::Mempool(e)
    }
}

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

impl From<StatusError> for SyncError {
    fn from(e: StatusError) -> Self {
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
