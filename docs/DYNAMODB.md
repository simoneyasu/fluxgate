# DynamoDB Persistence

## Access patterns

FluxGate needs one hot-path lookup: load or update the bucket for a `(policy, client key)` pair. It never scans buckets to make a rate-limit decision.

| Attribute | Type | Purpose |
| --- | --- | --- |
| `pk` | String | `RATE#<policy>#<SHA-256 client key>`; table partition key |
| `available_units` | Number | Exact quota-unit balance |
| `last_refill_ms` | Number | Epoch-millisecond algorithm timestamp |
| `version` | Number | Optimistic-concurrency generation |
| `expires_at` | Number | Epoch-second DynamoDB TTL cleanup time |

The raw client or API key is hashed before persistence. This is not authentication or encryption, but it prevents routine table inspection and logs from exposing credentials.

## Partition key design

The service retrieves buckets by exact identity, so a sort key or secondary index would add cost without serving an access pattern. DynamoDB distributes different hashed client keys across partitions. A single extremely popular key remains a hot item because strict per-key decisions must serialize somewhere.

## Repository boundary

The `BucketRepository` trait keeps the algorithm independent from AWS. It exposes:

- `load`: strongly consistent retrieval of one bucket;
- `compare_and_set`: create when absent, or replace only when `version` matches.

The DynamoDB adapter uses a `PutItem` condition expression for compare-and-set. `RateLimiter` wraps this primitive in a bounded loop that reloads and recomputes after conflicts.

## Concurrency behavior

If two service instances read version 7, only one conditional write can advance the item to version 8. The losing writer receives a conditional-check failure, reloads the winning state, and tries again. The retry budget and capped backoff prevent unbounded work on a hot key.

The integration suite distributes 200 simultaneous decisions across three independent AWS SDK clients sharing one capacity-100 bucket. Exactly 100 decisions are allowed and 100 are denied.

## TTL

`expires_at` uses epoch seconds, as required by DynamoDB TTL. Deletion is asynchronous and can occur after the timestamp. FluxGate always uses `last_refill_ms` for token arithmetic; an old item naturally refills to capacity even when DynamoDB has not deleted it.

The application creates the local table and enables TTL during startup.

## Credentials

When `DYNAMODB_ENDPOINT` is configured, the AWS loader uses SDK test credentials. Without an endpoint override, the normal AWS credential provider chain is used. No credentials are stored in the repository.
