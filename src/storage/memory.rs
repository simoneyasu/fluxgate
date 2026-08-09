use super::{BucketId, BucketRepository, RepositoryError, StoredBucket};
use crate::limiter::BucketState;
use async_trait::async_trait;
use std::{collections::HashMap, sync::RwLock};

/// Deterministic repository used by unit and HTTP tests.
#[derive(Default)]
pub struct MemoryRepository {
    buckets: RwLock<HashMap<BucketId, StoredBucket>>,
}

#[async_trait]
impl BucketRepository for MemoryRepository {
    async fn load(&self, id: &BucketId) -> Result<Option<StoredBucket>, RepositoryError> {
        Ok(self
            .buckets
            .read()
            .map_err(|error| RepositoryError::operation("memory_read", error))?
            .get(id)
            .copied())
    }

    async fn compare_and_set(
        &self,
        id: &BucketId,
        state: BucketState,
        expected_version: Option<u64>,
        expires_at_epoch_seconds: u64,
    ) -> Result<StoredBucket, RepositoryError> {
        let mut buckets = self
            .buckets
            .write()
            .map_err(|error| RepositoryError::operation("memory_write", error))?;
        let current = buckets.get(id).copied();
        if current.map(|bucket| bucket.version()) != expected_version {
            return Err(RepositoryError::Conflict);
        }

        let next_version = match expected_version {
            Some(version) => version.checked_add(1).ok_or(RepositoryError::Conflict)?,
            None => 0,
        };
        let stored = StoredBucket::new(state, next_version, expires_at_epoch_seconds);
        buckets.insert(id.clone(), stored);
        Ok(stored)
    }
}
