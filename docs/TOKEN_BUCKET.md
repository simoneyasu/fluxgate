# Exact Token-Bucket Arithmetic

The token bucket answers one question: given a policy, a previous bucket state, a request cost, and the current time, should the request be allowed and what state should be stored next?

It performs no I/O and does not read a clock, DynamoDB, an HTTP request, or environment variables. The transition is deterministic:

```text
(policy, previous state, cost, now) -> (decision, next state)
```

The storage layer loads a state, evaluates this function, and attempts to persist the returned state using optimistic concurrency control.

## Integer representation

Floating-point token counts can accumulate rounding errors. FluxGate instead uses policy-specific **quota units**:

- One token equals `refill_period_ms` quota units.
- Each elapsed millisecond adds `refill_tokens` quota units.
- A cost of `N` consumes `N * refill_period_ms` quota units.

For a policy that refills 100 tokens every 60,000 ms:

```text
1 token       = 60,000 quota units
1 elapsed ms  = 100 quota units
600 ms        = 60,000 quota units = exactly 1 token
```

This is the rational rate (`refill_tokens / refill_period_ms`) expressed with integers. Partial tokens remain in the state and are not discarded between requests.

Intermediate refill math uses `u128`; persisted state uses `u64`. Policy construction proves that the maximum bucket balance fits in `u64`, and the evaluator clamps every result to that validated capacity.

## Semantics

- A new bucket starts full.
- A denied request does not consume quota.
- `remaining` is the number of whole request units immediately available.
- `retry_after_ms` is present only on denial and means time until the rejected cost can succeed.
- `reset_after_ms` means time until the bucket is full.
- Time moving backward is an explicit error.
- TTL cleans inactive records but never determines whether a bucket is logically full.

## Separation from persistence

Keeping the algorithm independent from DynamoDB separates token arithmetic from distributed coordination. Algorithm edge cases are deterministic and require no database, while the repository integration tests prove that concurrent instances cannot overwrite each other's deductions.

## Alternatives

### Floating point

Simple to read, but equality boundaries and accumulated rounding are harder to reason about and test.

### Microtokens

Common and workable, but an arbitrary scale introduces a precision policy and can discard fractions smaller than that scale.

### Separate division remainder

Exact, but adds another state field and migration concern. Quota units retain the same information with fewer fields.

### Sliding window

Useful when bursts at window boundaries are unacceptable, but it requires more storage per key.
