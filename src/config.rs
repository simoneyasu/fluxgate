use std::{env, net::IpAddr};
use thiserror::Error;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: &str = "8080";
const DEFAULT_LOG_FILTER: &str = "fluxgate=info,tower_http=info";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub host: IpAddr,
    pub port: u16,
    pub log_filter: String,
    pub log_format: LogFormat,
    pub max_conflict_retries: u32,
    pub dynamodb: DynamoConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamoConfig {
    pub region: String,
    pub endpoint_url: Option<String>,
    pub table_name: String,
    pub bucket_ttl_seconds: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogFormat {
    Json,
    Pretty,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("APP_HOST must be a valid IP address, received {value:?}")]
    InvalidHost { value: String },
    #[error("APP_PORT must be a number from 1 to 65535, received {value:?}")]
    InvalidPort { value: String },
    #[error("LOG_FORMAT must be either 'json' or 'pretty', received {value:?}")]
    InvalidLogFormat { value: String },
    #[error("DYNAMODB_TABLE cannot be empty")]
    EmptyDynamoTable,
    #[error("BUCKET_TTL_SECONDS must be a positive integer, received {value:?}")]
    InvalidBucketTtl { value: String },
    #[error("MAX_CONFLICT_RETRIES must be an integer from 0 to 1000, received {value:?}")]
    InvalidConflictRetries { value: String },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let host_value = env::var("APP_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_owned());
        let port_value = env::var("APP_PORT").unwrap_or_else(|_| DEFAULT_PORT.to_owned());
        let log_filter = env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_owned());
        let log_format_value = env::var("LOG_FORMAT").unwrap_or_else(|_| "json".to_owned());
        let region = env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
        let endpoint_url = env::var("DYNAMODB_ENDPOINT")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let table_name =
            env::var("DYNAMODB_TABLE").unwrap_or_else(|_| "fluxgate-rate-limits".to_owned());
        let ttl_value = env::var("BUCKET_TTL_SECONDS").unwrap_or_else(|_| "86400".to_owned());
        let retries_value = env::var("MAX_CONFLICT_RETRIES").unwrap_or_else(|_| "32".to_owned());

        let host = host_value
            .parse()
            .map_err(|_| ConfigError::InvalidHost { value: host_value })?;
        let port = port_value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| ConfigError::InvalidPort {
                value: port_value.clone(),
            })?;
        let log_format = match log_format_value.to_ascii_lowercase().as_str() {
            "json" => LogFormat::Json,
            "pretty" => LogFormat::Pretty,
            _ => {
                return Err(ConfigError::InvalidLogFormat {
                    value: log_format_value,
                })
            }
        };
        if table_name.trim().is_empty() {
            return Err(ConfigError::EmptyDynamoTable);
        }
        let bucket_ttl_seconds = ttl_value
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| ConfigError::InvalidBucketTtl {
                value: ttl_value.clone(),
            })?;
        let max_conflict_retries = retries_value
            .parse::<u32>()
            .ok()
            .filter(|value| *value <= 1_000)
            .ok_or_else(|| ConfigError::InvalidConflictRetries {
                value: retries_value.clone(),
            })?;

        Ok(Self {
            host,
            port,
            log_filter,
            log_format,
            max_conflict_retries,
            dynamodb: DynamoConfig {
                region,
                endpoint_url,
                table_name,
                bucket_ttl_seconds,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert_eq!(
            DEFAULT_HOST.parse::<IpAddr>().expect("default host"),
            "0.0.0.0".parse::<IpAddr>().expect("literal host")
        );
        assert_eq!(DEFAULT_PORT.parse::<u16>().expect("default port"), 8080);
    }
}
