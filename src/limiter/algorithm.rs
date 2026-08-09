use super::models::{BucketState, Decision, Policy};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum LimiterError {
    #[error("bucket capacity must be greater than zero")]
    ZeroCapacity,
    #[error("refill_tokens must be greater than zero")]
    ZeroRefillTokens,
    #[error("refill_period_ms must be greater than zero")]
    ZeroRefillPeriod,
    #[error("policy values are too large to represent safely")]
    PolicyTooLarge,
    #[error("request cost must be greater than zero")]
    ZeroCost,
    #[error("request cost {cost} exceeds bucket capacity {capacity}")]
    CostExceedsCapacity { cost: u64, capacity: u64 },
    #[error("request cost is too large to represent safely")]
    CostTooLarge,
    #[error("calculated bucket value is too large to represent safely")]
    ArithmeticOverflow,
    #[error("current time {now_ms} precedes bucket timestamp {last_refill_ms}")]
    ClockMovedBackwards { now_ms: u64, last_refill_ms: u64 },
}

/// Applies one token-bucket transition at an explicit timestamp.
///
/// A missing state represents a new, full bucket. Rejected requests retain
/// their refilled balance; allowed requests atomically consume `cost` from the
/// returned state. Persistence and concurrency control are intentionally left
/// to later layers.
pub fn evaluate(
    policy: &Policy,
    state: Option<BucketState>,
    cost: u64,
    now_ms: u64,
) -> Result<Decision, LimiterError> {
    validate_cost(policy, cost)?;

    let state = state.unwrap_or_else(|| BucketState::full(policy, now_ms));
    if now_ms < state.last_refill_ms() {
        return Err(LimiterError::ClockMovedBackwards {
            now_ms,
            last_refill_ms: state.last_refill_ms(),
        });
    }

    let capacity_units = u128::from(policy.capacity_units());
    let elapsed_ms = u128::from(now_ms - state.last_refill_ms());
    let refilled_units = elapsed_ms * u128::from(policy.refill_tokens());
    let available_units =
        (u128::from(state.available_units()) + refilled_units).min(capacity_units);
    let cost_units = u128::from(policy.cost_units(cost)?);

    let allowed = available_units >= cost_units;
    let resulting_units = if allowed {
        available_units - cost_units
    } else {
        available_units
    };
    let remaining = resulting_units / u128::from(policy.refill_period_ms());
    let reset_after_ms = ceil_div(
        capacity_units - resulting_units,
        u128::from(policy.refill_tokens()),
    );
    let retry_after_ms = (!allowed).then(|| {
        ceil_div(
            cost_units - resulting_units,
            u128::from(policy.refill_tokens()),
        )
    });

    let resulting_units =
        u64::try_from(resulting_units).map_err(|_| LimiterError::ArithmeticOverflow)?;
    let remaining = u64::try_from(remaining).map_err(|_| LimiterError::ArithmeticOverflow)?;
    let reset_after_ms =
        u64::try_from(reset_after_ms).map_err(|_| LimiterError::ArithmeticOverflow)?;
    let retry_after_ms = retry_after_ms
        .map(|duration| u64::try_from(duration).map_err(|_| LimiterError::ArithmeticOverflow))
        .transpose()?;

    Ok(Decision::new(
        allowed,
        policy.capacity(),
        remaining,
        reset_after_ms,
        retry_after_ms,
        BucketState::from_parts(resulting_units, now_ms),
    ))
}

fn validate_cost(policy: &Policy, cost: u64) -> Result<(), LimiterError> {
    if cost == 0 {
        return Err(LimiterError::ZeroCost);
    }
    if cost > policy.capacity() {
        return Err(LimiterError::CostExceedsCapacity {
            cost,
            capacity: policy.capacity(),
        });
    }
    Ok(())
}

fn ceil_div(numerator: u128, denominator: u128) -> u128 {
    (numerator / denominator) + u128::from(!numerator.is_multiple_of(denominator))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(capacity: u64, refill_tokens: u64, period_ms: u64) -> Policy {
        Policy::new(capacity, refill_tokens, period_ms).expect("valid test policy")
    }

    fn spend_all(policy: &Policy, now_ms: u64) -> BucketState {
        evaluate(policy, None, policy.capacity(), now_ms)
            .expect("full-capacity request should succeed")
            .state()
    }

    #[test]
    fn first_request_starts_with_a_full_bucket() {
        let result = evaluate(&policy(100, 100, 60_000), None, 1, 10_000).unwrap();

        assert!(result.allowed());
        assert_eq!(result.limit(), 100);
        assert_eq!(result.remaining(), 99);
        assert_eq!(result.state().last_refill_ms(), 10_000);
    }

    #[test]
    fn consecutive_requests_consume_tokens() {
        let policy = policy(10, 10, 1_000);
        let first = evaluate(&policy, None, 3, 0).unwrap();
        let second = evaluate(&policy, Some(first.state()), 2, 0).unwrap();

        assert!(second.allowed());
        assert_eq!(second.remaining(), 5);
    }

    #[test]
    fn bucket_can_reach_exactly_zero() {
        let result = evaluate(&policy(10, 10, 1_000), None, 10, 0).unwrap();

        assert!(result.allowed());
        assert_eq!(result.remaining(), 0);
        assert_eq!(result.state().available_units(), 0);
    }

    #[test]
    fn request_is_rejected_at_zero_tokens() {
        let policy = policy(10, 10, 1_000);
        let empty = spend_all(&policy, 0);
        let result = evaluate(&policy, Some(empty), 1, 0).unwrap();

        assert!(!result.allowed());
        assert_eq!(result.remaining(), 0);
        assert_eq!(result.retry_after_ms(), Some(100));
    }

    #[test]
    fn partial_refill_is_retained_exactly() {
        let policy = policy(10, 10, 1_000);
        let empty = spend_all(&policy, 0);
        let result = evaluate(&policy, Some(empty), 2, 250).unwrap();

        assert!(result.allowed());
        assert_eq!(result.remaining(), 0);
        assert_eq!(result.state().available_units(), 500);
        assert_eq!(result.reset_after_ms(), 950);
    }

    #[test]
    fn enough_elapsed_time_fully_refills_bucket() {
        let policy = policy(10, 10, 1_000);
        let empty = spend_all(&policy, 0);
        let result = evaluate(&policy, Some(empty), 1, 1_000).unwrap();

        assert!(result.allowed());
        assert_eq!(result.remaining(), 9);
    }

    #[test]
    fn refill_never_exceeds_capacity() {
        let policy = policy(10, 10, 1_000);
        let almost_full = evaluate(&policy, None, 1, 0).unwrap().state();
        let result = evaluate(&policy, Some(almost_full), 1, 10_000).unwrap();

        assert_eq!(result.remaining(), 9);
        assert_eq!(result.state().available_units(), 9_000);
    }

    #[test]
    fn request_cost_can_consume_multiple_tokens() {
        let result = evaluate(&policy(20, 20, 1_000), None, 7, 0).unwrap();

        assert!(result.allowed());
        assert_eq!(result.remaining(), 13);
    }

    #[test]
    fn cost_larger_than_capacity_is_invalid() {
        let result = evaluate(&policy(5, 5, 1_000), None, 6, 0);

        assert_eq!(
            result,
            Err(LimiterError::CostExceedsCapacity {
                cost: 6,
                capacity: 5
            })
        );
    }

    #[test]
    fn refill_on_exact_period_boundary_is_exact() {
        let policy = policy(4, 1, 1_000);
        let empty = spend_all(&policy, 100);
        let result = evaluate(&policy, Some(empty), 1, 1_100).unwrap();

        assert!(result.allowed());
        assert_eq!(result.state().available_units(), 0);
        assert_eq!(result.retry_after_ms(), None);
    }

    #[test]
    fn long_inactivity_safely_clamps_to_capacity() {
        let policy = policy(100, 100, 60_000);
        let empty = spend_all(&policy, 0);
        let result = evaluate(&policy, Some(empty), 1, u64::MAX).unwrap();

        assert!(result.allowed());
        assert_eq!(result.remaining(), 99);
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        assert_eq!(Policy::new(0, 1, 1), Err(LimiterError::ZeroCapacity));
        assert_eq!(Policy::new(1, 0, 1), Err(LimiterError::ZeroRefillTokens));
        assert_eq!(Policy::new(1, 1, 0), Err(LimiterError::ZeroRefillPeriod));
        assert_eq!(
            Policy::new(u64::MAX, 1, 2),
            Err(LimiterError::PolicyTooLarge)
        );
        assert_eq!(
            evaluate(&policy(1, 1, 1_000), None, 0, 0),
            Err(LimiterError::ZeroCost)
        );
    }

    #[test]
    fn time_cannot_move_backwards() {
        let policy = policy(10, 10, 1_000);
        let state = BucketState::full(&policy, 500);

        assert_eq!(
            evaluate(&policy, Some(state), 1, 499),
            Err(LimiterError::ClockMovedBackwards {
                now_ms: 499,
                last_refill_ms: 500
            })
        );
    }

    #[test]
    fn rejected_request_preserves_refilled_balance() {
        let policy = policy(10, 10, 1_000);
        let empty = spend_all(&policy, 0);
        let result = evaluate(&policy, Some(empty), 5, 250).unwrap();

        assert!(!result.allowed());
        assert_eq!(result.state().available_units(), 2_500);
        assert_eq!(result.retry_after_ms(), Some(250));
    }

    #[test]
    fn oversized_persisted_balance_is_defensively_clamped() {
        let policy = policy(10, 10, 1_000);
        let corrupted = BucketState::from_parts(u64::MAX, 0);
        let result = evaluate(&policy, Some(corrupted), 1, 0).unwrap();

        assert_eq!(result.remaining(), 9);
    }
}
