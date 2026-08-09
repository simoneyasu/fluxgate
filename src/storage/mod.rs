//! Persistence boundary for rate-limit bucket state.

mod dynamodb;
mod memory;
mod models;

use async_trait::async_trait;
use thiserror::Error;

pub use dynamodb::DynamoRepository;
pub use memory::MemoryRepository;
pub use models::{BucketId, BucketIdError, StoredBucket};

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("the bucket changed before this write could be committed")]
    Conflict,
    #[error("stored bucket is missing or contains an invalid {field} attribute")]
    InvalidItem { field: &'static str },
    #[error("failed to construct DynamoDB schema: {0}")]
    Schema(String),
    #[error("DynamoDB operation {operation} failed: {message}")]
    Operation {
        operation: &'static str,
        message: String,
    },
    #[error("DynamoDB table did not become active before the startup deadline")]
    TableStartupTimeout,
}

impl RepositoryError {
    fn operation(operation: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Operation {
            operation,
            message: error.to_string(),
        }
    }
}

/// Storage operations needed by the rate-limiter service.
///
/// `compare_and_set` creates when `expected_version` is `None`, or replaces an
/// existing item only when its version still matches. The service builds a
/// bounded retry loop around this atomic primitive.
#[async_trait]
pub trait BucketRepository: Send + Sync {
    async fn load(&self, id: &BucketId) -> Result<Option<StoredBucket>, RepositoryError>;

    async fn compare_and_set(
        &self,
        id: &BucketId,
        state: crate::limiter::BucketState,
        expected_version: Option<u64>,
        expires_at_epoch_seconds: u64,
    ) -> Result<StoredBucket, RepositoryError>;
}
