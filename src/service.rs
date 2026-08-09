//! Distributed orchestration for the pure token-bucket transition.

use crate::{
    limiter::{evaluate, Decision, LimiterError, Policy},
    storage::{BucketId, BucketIdError, BucketRepository, RepositoryError},
};
use std::{sync::Arc, time::Duration};
use thiserror::Error;

#[derive(Clone)]
pub struct RateLimiter {
    repository: Arc<dyn BucketRepository>,
    max_conflict_retries: u32,
    bucket_ttl_seconds: u64,
}

#[derive(Debug, Error)]
pub enum RateLimiterError {
    #[error(transparent)]
    InvalidBucketId(#[from] BucketIdError),
    #[error(transparent)]
    InvalidRequest(#[from] LimiterError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("bucket expiration timestamp overflowed")]
    ExpiryOverflow,
    #[error("bucket remained contended after {attempts} attempts")]
    ContentionExhausted { attempts: u32 },
}

impl RateLimiter {
    pub fn new(
        repository: Arc<dyn BucketRepository>,
        max_conflict_retries: u32,
        bucket_ttl_seconds: u64,
    ) -> Self {
        Self {
            repository,
            max_conflict_retries,
            bucket_ttl_seconds,
        }
    }

    /// Checks and consumes quota using a bounded optimistic-concurrency loop.
    pub async fn check(
        &self,
        policy_name: &str,
        client_key: &str,
        policy: &Policy,
        cost: u64,
        now_ms: u64,
    ) -> Result<Decision, RateLimiterError> {
        let id = BucketId::new(policy_name, client_key)?;
        let expires_at = (now_ms / 1_000)
            .checked_add(self.bucket_ttl_seconds)
            .ok_or(RateLimiterError::ExpiryOverflow)?;

        for attempt in 0..=self.max_conflict_retries {
            let current = self.repository.load(&id).await?;
            let decision = evaluate(policy, current.map(|stored| stored.state()), cost, now_ms)?;
            let expected_version = current.map(|stored| stored.version());

            match self
                .repository
                .compare_and_set(&id, decision.state(), expected_version, expires_at)
                .await
            {
                Ok(_) => return Ok(decision),
                Err(RepositoryError::Conflict) if attempt < self.max_conflict_retries => {
                    tracing::debug!(
                        policy = policy_name,
                        attempt = attempt + 1,
                        "conditional write conflicted; retrying"
                    );
                    backoff(attempt).await;
                }
                Err(RepositoryError::Conflict) => {
                    tracing::warn!(
                        policy = policy_name,
                        attempts = attempt + 1,
                        "conditional write retry budget exhausted"
                    );
                    return Err(RateLimiterError::ContentionExhausted {
                        attempts: attempt + 1,
                    });
                }
                Err(error) => return Err(error.into()),
            }
        }

        Err(RateLimiterError::ContentionExhausted {
            attempts: self.max_conflict_retries.saturating_add(1),
        })
    }
}

async fn backoff(attempt: u32) {
    if attempt == 0 {
        tokio::task::yield_now().await;
        return;
    }
    let micros = u64::from(attempt).saturating_mul(100).min(5_000);
    tokio::time::sleep(Duration::from_micros(micros)).await;
}
