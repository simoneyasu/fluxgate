use crate::limiter::{LimiterError, Policy};
use std::collections::HashMap;

/// Immutable, configuration-backed policies exposed by the API.
#[derive(Clone)]
pub struct PolicyRegistry {
    policies: HashMap<&'static str, Policy>,
}

impl PolicyRegistry {
    /// Builds the validated policies shipped with the service.
    pub fn built_in() -> Result<Self, LimiterError> {
        Ok(Self {
            policies: HashMap::from([
                ("default", Policy::new(100, 100, 60_000)?),
                ("login", Policy::new(10, 10, 60_000)?),
                ("expensive_api", Policy::new(20, 20, 300_000)?),
            ]),
        })
    }

    pub fn get(&self, name: &str) -> Option<Policy> {
        self.policies.get(name).copied()
    }
}
