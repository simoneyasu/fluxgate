use crate::limiter::BucketState;
use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_POLICY_LENGTH: usize = 64;
const MAX_CLIENT_KEY_LENGTH: usize = 256;

/// Collision-resistant, privacy-conscious DynamoDB identifier for one bucket.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BucketId {
    storage_key: String,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BucketIdError {
    #[error("policy name cannot be empty")]
    EmptyPolicy,
    #[error("policy name must contain at most {MAX_POLICY_LENGTH} bytes")]
    PolicyTooLong,
    #[error("policy name may contain only ASCII letters, digits, underscores, and hyphens")]
    InvalidPolicy,
    #[error("client key cannot be empty")]
    EmptyClientKey,
    #[error("client key must contain at most {MAX_CLIENT_KEY_LENGTH} bytes")]
    ClientKeyTooLong,
}

impl BucketId {
    pub fn new(policy: &str, client_key: &str) -> Result<Self, BucketIdError> {
        if policy.is_empty() {
            return Err(BucketIdError::EmptyPolicy);
        }
        if policy.len() > MAX_POLICY_LENGTH {
            return Err(BucketIdError::PolicyTooLong);
        }
        if !policy
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(BucketIdError::InvalidPolicy);
        }
        if client_key.is_empty() {
            return Err(BucketIdError::EmptyClientKey);
        }
        if client_key.len() > MAX_CLIENT_KEY_LENGTH {
            return Err(BucketIdError::ClientKeyTooLong);
        }

        let digest = hex::encode(Sha256::digest(client_key.as_bytes()));
        Ok(Self {
            storage_key: format!("RATE#{policy}#{digest}"),
        })
    }

    pub fn storage_key(&self) -> &str {
        &self.storage_key
    }
}

/// Bucket state plus storage metadata used for optimistic concurrency and TTL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoredBucket {
    state: BucketState,
    version: u64,
    expires_at_epoch_seconds: u64,
}

impl StoredBucket {
    pub(crate) fn new(state: BucketState, version: u64, expires_at_epoch_seconds: u64) -> Self {
        Self {
            state,
            version,
            expires_at_epoch_seconds,
        }
    }

    pub fn state(&self) -> BucketState {
        self.state
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn expires_at_epoch_seconds(&self) -> u64 {
        self.expires_at_epoch_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_key_is_not_exposed_in_storage_key() {
        let id = BucketId::new("login", "secret-api-key").unwrap();

        assert!(id.storage_key().starts_with("RATE#login#"));
        assert!(!id.storage_key().contains("secret-api-key"));
    }

    #[test]
    fn policies_partition_identical_client_keys() {
        let login = BucketId::new("login", "user-1").unwrap();
        let default = BucketId::new("default", "user-1").unwrap();

        assert_ne!(login, default);
    }

    #[test]
    fn invalid_identifiers_are_rejected() {
        assert_eq!(BucketId::new("", "key"), Err(BucketIdError::EmptyPolicy));
        assert_eq!(
            BucketId::new("bad policy", "key"),
            Err(BucketIdError::InvalidPolicy)
        );
        assert_eq!(
            BucketId::new("default", ""),
            Err(BucketIdError::EmptyClientKey)
        );
    }
}
