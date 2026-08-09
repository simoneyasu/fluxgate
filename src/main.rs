use fluxgate::{
    api::AppState, app, config::Config, policies::PolicyRegistry, service::RateLimiter,
    storage::DynamoRepository, telemetry,
};
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = Config::from_env()?;
    telemetry::init(&config)?;

    let repository = DynamoRepository::connect(&config.dynamodb).await?;
    repository.ensure_table().await?;
    tracing::info!(table = repository.table_name(), "DynamoDB table is ready");
    let limiter = RateLimiter::new(
        Arc::new(repository),
        config.max_conflict_retries,
        config.dynamodb.bucket_ttl_seconds,
    );
    let state = AppState {
        limiter,
        policies: PolicyRegistry::built_in()?,
    };

    let address = SocketAddr::from((config.host, config.port));
    let listener = TcpListener::bind(address).await?;
    tracing::info!(%address, "FluxGate is listening");

    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
    tracing::info!("shutdown signal received");
}
