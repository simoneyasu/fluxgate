use fluxgate::{
    config::DynamoConfig,
    limiter::{BucketState, Policy},
    service::RateLimiter,
    storage::{BucketId, BucketRepository, DynamoRepository, RepositoryError},
};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinSet;

fn test_config() -> DynamoConfig {
    DynamoConfig {
        region: "us-east-1".to_owned(),
        endpoint_url: Some(
            std::env::var("DYNAMODB_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:8000".to_owned()),
        ),
        table_name: "fluxgate-integration".to_owned(),
        bucket_ttl_seconds: 86_400,
    }
}

fn unique_key(label: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_nanos();
    format!("{label}-{}-{nanos}", std::process::id())
}

#[tokio::test]
#[ignore = "requires DynamoDB Local on port 8000"]
async fn persists_updates_and_isolates_buckets() {
    let repository = DynamoRepository::connect(&test_config()).await.unwrap();
    repository.ensure_table().await.unwrap();

    let first = BucketId::new("default", &unique_key("first")).unwrap();
    let second = BucketId::new("login", &unique_key("second")).unwrap();
    let expires_at = 2_000_000_000;

    let created = repository
        .compare_and_set(
            &first,
            BucketState::from_parts(9_000, 1_000),
            None,
            expires_at,
        )
        .await
        .unwrap();
    repository
        .compare_and_set(
            &second,
            BucketState::from_parts(2_000, 2_000),
            None,
            expires_at + 1,
        )
        .await
        .unwrap();

    let loaded = repository.load(&first).await.unwrap().unwrap();
    assert_eq!(loaded, created);
    assert_eq!(loaded.version(), 0);
    assert_eq!(loaded.expires_at_epoch_seconds(), expires_at);
    assert_eq!(loaded.state(), BucketState::from_parts(9_000, 1_000));

    let updated = repository
        .compare_and_set(
            &first,
            BucketState::from_parts(7_500, 1_500),
            Some(loaded.version()),
            expires_at + 60,
        )
        .await
        .unwrap();
    assert_eq!(updated.version(), 1);
    assert_eq!(repository.load(&first).await.unwrap(), Some(updated));

    let independent = repository.load(&second).await.unwrap().unwrap();
    assert_eq!(independent.state(), BucketState::from_parts(2_000, 2_000));
    assert_eq!(independent.expires_at_epoch_seconds(), expires_at + 1);
}

#[tokio::test]
#[ignore = "requires DynamoDB Local on port 8000"]
async fn stale_version_is_rejected() {
    let repository = DynamoRepository::connect(&test_config()).await.unwrap();
    repository.ensure_table().await.unwrap();
    let id = BucketId::new("default", &unique_key("conflict")).unwrap();

    repository
        .compare_and_set(&id, BucketState::from_parts(10, 10), None, 2_000_000_000)
        .await
        .unwrap();
    repository
        .compare_and_set(&id, BucketState::from_parts(9, 11), Some(0), 2_000_000_001)
        .await
        .unwrap();

    let result = repository
        .compare_and_set(&id, BucketState::from_parts(8, 12), Some(0), 2_000_000_002)
        .await;
    assert!(matches!(result, Err(RepositoryError::Conflict)));
}

#[tokio::test]
#[ignore = "requires DynamoDB Local on port 8000"]
async fn two_hundred_concurrent_checks_allow_exactly_capacity() {
    let config = test_config();
    let repository = DynamoRepository::connect(&config).await.unwrap();
    repository.ensure_table().await.unwrap();

    let limiters = [
        RateLimiter::new(
            Arc::new(DynamoRepository::connect(&config).await.unwrap()),
            512,
            86_400,
        ),
        RateLimiter::new(
            Arc::new(DynamoRepository::connect(&config).await.unwrap()),
            512,
            86_400,
        ),
        RateLimiter::new(
            Arc::new(DynamoRepository::connect(&config).await.unwrap()),
            512,
            86_400,
        ),
    ];
    let policy = Policy::new(100, 100, 60_000).unwrap();
    let key = unique_key("hot-key");
    let now_ms = 1_800_000_000_000;
    let mut tasks = JoinSet::new();

    for index in 0..200 {
        let limiter = limiters[index % limiters.len()].clone();
        let key = key.clone();
        tasks.spawn(async move { limiter.check("default", &key, &policy, 1, now_ms).await });
    }

    let mut allowed = 0;
    let mut denied = 0;
    while let Some(result) = tasks.join_next().await {
        let decision = result
            .expect("task should not panic")
            .expect("check should complete");
        if decision.allowed() {
            allowed += 1;
        } else {
            denied += 1;
        }
    }

    assert_eq!(allowed, 100);
    assert_eq!(denied, 100);

    let id = BucketId::new("default", &key).unwrap();
    let final_state = repository.load(&id).await.unwrap().unwrap();
    assert_eq!(final_state.version(), 199);
    assert_eq!(final_state.state().available_units(), 0);
}
