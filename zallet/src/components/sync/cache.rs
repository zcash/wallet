use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use zcash_client_backend::{
    data_api::{
        chain::{BlockCache, BlockSource, error::Error as ChainError},
        scanning::ScanRange,
    },
    proto::compact_formats::CompactBlock,
};
use zcash_protocol::consensus::BlockHeight;

use super::SyncError;

#[derive(Debug)]
pub(super) struct MemoryCache {
    blocks: Arc<RwLock<BTreeMap<BlockHeight, CompactBlock>>>,
}

impl MemoryCache {
    /// Constructs a new in-memory [`CompactBlock`] cache.
    pub(super) fn new() -> Self {
        Self {
            blocks: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl BlockSource for MemoryCache {
    type Error = SyncError;

    fn with_blocks<F, WalletErrT>(
        &self,
        from_height: Option<BlockHeight>,
        limit: Option<usize>,
        mut with_block: F,
    ) -> Result<(), ChainError<WalletErrT, Self::Error>>
    where
        F: FnMut(CompactBlock) -> Result<(), ChainError<WalletErrT, Self::Error>>,
    {
        tokio::task::block_in_place(|| {
            for (i, (_, block)) in self
                .blocks
                .blocking_read()
                .iter()
                .skip_while(|(h, _)| from_height.is_some_and(|from_height| **h < from_height))
                .enumerate()
            {
                if limit.is_none_or(|limit| i < limit) {
                    // The `BlockSource` trait does not guarantee sequential blocks.
                    with_block(block.clone())?;
                } else {
                    break;
                }
            }
            Ok(())
        })
    }
}

#[async_trait]
impl BlockCache for MemoryCache {
    fn get_tip_height(
        &self,
        range: Option<&ScanRange>,
    ) -> Result<Option<BlockHeight>, Self::Error> {
        tokio::task::block_in_place(|| {
            Ok(if let Some(range) = range {
                self.blocks
                    .blocking_read()
                    .iter()
                    .rev()
                    .filter(|(height, _)| range.block_range().contains(height))
                    .map(|(height, _)| *height)
                    .next()
            } else {
                self.blocks
                    .blocking_read()
                    .last_key_value()
                    .map(|(height, _)| *height)
            })
        })
    }

    async fn read(&self, range: &ScanRange) -> Result<Vec<CompactBlock>, Self::Error> {
        Ok(self
            .blocks
            .read()
            .await
            .iter()
            .filter(|(height, _)| range.block_range().contains(height))
            .map(|(_, block)| block.clone())
            .collect())
    }

    async fn insert(&self, compact_blocks: Vec<CompactBlock>) -> Result<(), Self::Error> {
        for block in compact_blocks {
            self.blocks.write().await.insert(block.height(), block);
        }
        Ok(())
    }

    async fn delete(&self, range: ScanRange) -> Result<(), Self::Error> {
        let mut height = range.block_range().start;
        while height < range.block_range().end {
            self.blocks.write().await.remove(&height);
            height = height + 1;
        }
        Ok(())
    }
}
