//! Pure rate-limiting domain logic.
//!
//! This module deliberately has no HTTP, clock, or database dependencies. A
//! caller supplies the current time and persisted state, making every state
//! transition deterministic and straightforward to test.

mod algorithm;
mod models;

pub use algorithm::{evaluate, LimiterError};
pub use models::{BucketState, Decision, Policy};
