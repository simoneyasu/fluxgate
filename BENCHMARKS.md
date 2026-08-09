# Benchmarks

FluxGate includes reproducible k6 workloads for measuring throughput and latency under different key distributions. Run identifiers avoid reusing a previously depleted bucket.

```bash
RUN_ID=$(date +%s) SCENARIO=single_key k6 run load/k6.js
RUN_ID=$(date +%s) SCENARIO=hundred_keys k6 run load/k6.js
RUN_ID=$(date +%s) SCENARIO=thousand_keys k6 run load/k6.js
RUN_ID=$(date +%s) SCENARIO=hot_key k6 run load/k6.js
```

Each profile emits `allowed_decisions`, `denied_decisions`, and `api_errors` counters in addition to k6's throughput and median/p95/p99 latency summary. Record the environment, CPU architecture, DynamoDB mode, application instance count, and complete raw summary. Do not compare local DynamoDB latency directly with AWS DynamoDB.

## Results

Benchmark results should include the raw k6 summary and a description of the test environment.
