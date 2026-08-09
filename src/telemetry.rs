use crate::config::{Config, LogFormat};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Installs FluxGate's process-wide tracing subscriber.
pub fn init(config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter = EnvFilter::try_new(&config.log_filter)?;
    let registry = tracing_subscriber::registry().with(filter);

    match config.log_format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .try_init()?,
        LogFormat::Pretty => registry
            .with(tracing_subscriber::fmt::layer().pretty())
            .try_init()?,
    }

    Ok(())
}
