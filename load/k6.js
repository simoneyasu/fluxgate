import http from "k6/http";
import { check } from "k6";
import { Counter } from "k6/metrics";

const allowedDecisions = new Counter("allowed_decisions");
const deniedDecisions = new Counter("denied_decisions");
const apiErrors = new Counter("api_errors");

const profiles = {
  single_key: { vus: 20, iterations: 1_000 },
  hundred_keys: { vus: 50, iterations: 10_000 },
  thousand_keys: { vus: 100, iterations: 20_000 },
  hot_key: { vus: 200, iterations: 10_000 },
};

const scenario = __ENV.SCENARIO || "single_key";
if (!profiles[scenario]) {
  throw new Error(`Unknown SCENARIO ${scenario}`);
}

export const options = {
  ...profiles[scenario],
  thresholds: {
    http_req_failed: ["rate==0"],
    http_req_duration: ["p(95)<1000"],
  },
};

export default function () {
  let key;
  if (scenario === "hot_key" || scenario === "single_key") {
    key = `${scenario}-${__ENV.RUN_ID || "local"}`;
  } else if (scenario === "hundred_keys") {
    key = `user-${__ITER % 100}-${__ENV.RUN_ID || "local"}`;
  } else {
    key = `user-${__ITER % 1000}-${__ENV.RUN_ID || "local"}`;
  }

  const response = http.post(
    `${__ENV.BASE_URL || "http://localhost:8080"}/v1/check`,
    JSON.stringify({ key }),
    { headers: { "Content-Type": "application/json" } },
  );
  if (response.status !== 200) {
    apiErrors.add(1);
  } else {
    const body = response.json();
    if (body.allowed) {
      allowedDecisions.add(1);
    } else {
      deniedDecisions.add(1);
    }
  }
  check(response, {
    "status is 200": (result) => result.status === 200,
    "decision is present": (result) => {
      const body = result.json();
      return typeof body.allowed === "boolean";
    },
  });
}
