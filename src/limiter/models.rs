use super::algorithm::LimiterError;
use serde::{Deserialize, Serialize};

/// Static configuration for one token-bucket policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Policy {
    capacity: u64,
    refill_tokens: u64,
    refill_period_ms: u64,
}

impl Policy {
    /// Creates a validated policy.
    pub fn new(
        capacity: u64,
        refill_tokens: u64,
        refill_period_ms: u64,
    ) -> Result<Self, LimiterError> {
        if capacity == 0 {
            return Err(LimiterError::ZeroCapacity);
        }
        if refill_tokens == 0 {
            return Err(LimiterError::ZeroRefillTokens);
        }
        if refill_period_ms == 0 {
            return Err(LimiterError::ZeroRefillPeriod);
        }
        capacity
            .checked_mul(refill_period_ms)
            .ok_or(LimiterError::PolicyTooLarge)?;

        Ok(Self {
            capacity,
            refill_tokens,
            refill_period_ms,
        })
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    pub fn refill_tokens(&self) -> u64 {
        self.refill_tokens
    }

    pub fn refill_period_ms(&self) -> u64 {
        self.refill_period_ms
    }

    pub(crate) fn capacity_units(&self) -> u64 {
        // Only `new` can construct a Policy, and it proves this fits in u64.
        self.capacity * self.refill_period_ms
    }

    pub(crate) fn cost_units(&self, cost: u64) -> Result<u64, LimiterError> {
        cost.checked_mul(self.refill_period_ms)
            .ok_or(LimiterError::CostTooLarge)
    }
}

/// Mutable token-bucket state suitable for persistence.
///
/// One whole token equals `policy.refill_period_ms()` quota units. Each elapsed
/// millisecond adds `policy.refill_tokens()` units. This rational representation
/// retains partial tokens exactly without floating-point arithmetic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BucketState {
    available_units: u64,
    last_refill_ms: u64,
}

impl BucketState {
    /// Creates a full bucket at the supplied timestamp.
    pub fn full(policy: &Policy, now_ms: u64) -> Self {
        Self {
            available_units: policy.capacity_units(),
            last_refill_ms: now_ms,
        }
    }

    /// Reconstructs a bucket loaded from storage.
    ///
    /// The evaluator defensively clamps `available_units` to the policy's
    /// capacity, so stale or oversized persisted values cannot grant excess
    /// quota.
    pub fn from_parts(available_units: u64, last_refill_ms: u64) -> Self {
        Self {
            available_units,
            last_refill_ms,
        }
    }

    pub fn available_units(&self) -> u64 {
        self.available_units
    }

    pub fn last_refill_ms(&self) -> u64 {
        self.last_refill_ms
    }
}

/// Result of one deterministic token-bucket transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Decision {
    allowed: bool,
    limit: u64,
    remaining: u64,
    reset_after_ms: u64,
    retry_after_ms: Option<u64>,
    state: BucketState,
}

impl Decision {
    pub(crate) fn new(
        allowed: bool,
        limit: u64,
        remaining: u64,
        reset_after_ms: u64,
        retry_after_ms: Option<u64>,
        state: BucketState,
    ) -> Self {
        Self {
            allowed,
            limit,
            remaining,
            reset_after_ms,
            retry_after_ms,
            state,
        }
    }

    pub fn allowed(&self) -> bool {
        self.allowed
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Whole request units immediately available after this decision.
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    /// Milliseconds until the bucket is full again.
    pub fn reset_after_ms(&self) -> u64 {
        self.reset_after_ms
    }

    /// Milliseconds until this rejected cost can succeed.
    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }

    pub fn state(&self) -> BucketState {
        self.state
    }
}
