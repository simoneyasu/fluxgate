use super::{BucketId, BucketRepository, RepositoryError, StoredBucket};
use crate::{config::DynamoConfig, limiter::BucketState};
use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::{
        create_table::CreateTableError, describe_table::DescribeTableError, put_item::PutItemError,
    },
    types::{
        AttributeDefinition, AttributeValue, BillingMode, KeySchemaElement, KeyType,
        ScalarAttributeType, TableStatus, TimeToLiveSpecification, TimeToLiveStatus,
    },
    Client,
};
use std::{collections::HashMap, time::Duration};

const PRIMARY_KEY: &str = "pk";
const TABLE_WAIT_ATTEMPTS: usize = 50;

#[derive(Clone)]
pub struct DynamoRepository {
    client: Client,
    table_name: String,
}

impl DynamoRepository {
    pub async fn connect(config: &DynamoConfig) -> Result<Self, RepositoryError> {
        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()));
        if let Some(endpoint_url) = &config.endpoint_url {
            loader = loader.test_credentials().endpoint_url(endpoint_url);
        }
        let shared_config = loader.load().await;

        Ok(Self {
            client: Client::new(&shared_config),
            table_name: config.table_name.clone(),
        })
    }

    pub fn from_client(client: Client, table_name: impl Into<String>) -> Self {
        Self {
            client,
            table_name: table_name.into(),
        }
    }

    pub fn table_name(&self) -> &str {
        &self.table_name
    }

    pub async fn ensure_table(&self) -> Result<(), RepositoryError> {
        match self
            .client
            .describe_table()
            .table_name(&self.table_name)
            .send()
            .await
        {
            Ok(_) => {
                self.wait_until_active().await?;
                return self.ensure_ttl().await;
            }
            Err(error) if is_missing_table(&error) => {}
            Err(error) => return Err(RepositoryError::operation("describe_table", error)),
        }

        let attribute = AttributeDefinition::builder()
            .attribute_name(PRIMARY_KEY)
            .attribute_type(ScalarAttributeType::S)
            .build()
            .map_err(|error| RepositoryError::Schema(error.to_string()))?;
        let key = KeySchemaElement::builder()
            .attribute_name(PRIMARY_KEY)
            .key_type(KeyType::Hash)
            .build()
            .map_err(|error| RepositoryError::Schema(error.to_string()))?;

        match self
            .client
            .create_table()
            .table_name(&self.table_name)
            .attribute_definitions(attribute)
            .key_schema(key)
            .billing_mode(BillingMode::PayPerRequest)
            .send()
            .await
        {
            Ok(_) => {}
            Err(error) if is_table_already_being_created(&error) => {}
            Err(error) => return Err(RepositoryError::operation("create_table", error)),
        }

        self.wait_until_active().await?;
        self.ensure_ttl().await
    }

    async fn wait_until_active(&self) -> Result<(), RepositoryError> {
        for _ in 0..TABLE_WAIT_ATTEMPTS {
            let output = self
                .client
                .describe_table()
                .table_name(&self.table_name)
                .send()
                .await
                .map_err(|error| RepositoryError::operation("describe_table", error))?;
            if output
                .table()
                .and_then(|table| table.table_status())
                .is_some_and(|status| status == &TableStatus::Active)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(RepositoryError::TableStartupTimeout)
    }

    async fn ensure_ttl(&self) -> Result<(), RepositoryError> {
        let output = self
            .client
            .describe_time_to_live()
            .table_name(&self.table_name)
            .send()
            .await
            .map_err(|error| RepositoryError::operation("describe_time_to_live", error))?;
        let status = output
            .time_to_live_description()
            .and_then(|description| description.time_to_live_status());
        if matches!(
            status,
            Some(TimeToLiveStatus::Enabled | TimeToLiveStatus::Enabling)
        ) {
            return Ok(());
        }

        let specification = TimeToLiveSpecification::builder()
            .attribute_name("expires_at")
            .enabled(true)
            .build()
            .map_err(|error| RepositoryError::Schema(error.to_string()))?;
        self.client
            .update_time_to_live()
            .table_name(&self.table_name)
            .time_to_live_specification(specification)
            .send()
            .await
            .map_err(|error| RepositoryError::operation("update_time_to_live", error))?;
        Ok(())
    }
}

#[async_trait]
impl BucketRepository for DynamoRepository {
    async fn load(&self, id: &BucketId) -> Result<Option<StoredBucket>, RepositoryError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key(PRIMARY_KEY, AttributeValue::S(id.storage_key().to_owned()))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| RepositoryError::operation("get_item", error))?;

        output.item.map(decode_item).transpose()
    }

    async fn compare_and_set(
        &self,
        id: &BucketId,
        state: BucketState,
        expected_version: Option<u64>,
        expires_at_epoch_seconds: u64,
    ) -> Result<StoredBucket, RepositoryError> {
        let next_version = expected_version
            .map(|version| version.checked_add(1).ok_or(RepositoryError::Conflict))
            .transpose()?
            .unwrap_or(0);
        let item = encode_item(id, state, next_version, expires_at_epoch_seconds);
        let mut request = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item));

        request = match expected_version {
            Some(version) => request
                .condition_expression("#version = :expected_version")
                .expression_attribute_names("#version", "version")
                .expression_attribute_values(
                    ":expected_version",
                    AttributeValue::N(version.to_string()),
                ),
            None => request
                .condition_expression("attribute_not_exists(#pk)")
                .expression_attribute_names("#pk", PRIMARY_KEY),
        };

        request.send().await.map_err(|error| {
            if is_conditional_conflict(&error) {
                RepositoryError::Conflict
            } else {
                RepositoryError::operation("put_item", error)
            }
        })?;

        Ok(StoredBucket::new(
            state,
            next_version,
            expires_at_epoch_seconds,
        ))
    }
}

fn encode_item(
    id: &BucketId,
    state: BucketState,
    version: u64,
    expires_at_epoch_seconds: u64,
) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (
            PRIMARY_KEY.to_owned(),
            AttributeValue::S(id.storage_key().to_owned()),
        ),
        (
            "available_units".to_owned(),
            AttributeValue::N(state.available_units().to_string()),
        ),
        (
            "last_refill_ms".to_owned(),
            AttributeValue::N(state.last_refill_ms().to_string()),
        ),
        ("version".to_owned(), AttributeValue::N(version.to_string())),
        (
            "expires_at".to_owned(),
            AttributeValue::N(expires_at_epoch_seconds.to_string()),
        ),
    ])
}

fn decode_item(item: HashMap<String, AttributeValue>) -> Result<StoredBucket, RepositoryError> {
    let available_units = number(&item, "available_units")?;
    let last_refill_ms = number(&item, "last_refill_ms")?;
    let version = number(&item, "version")?;
    let expires_at = number(&item, "expires_at")?;

    Ok(StoredBucket::new(
        BucketState::from_parts(available_units, last_refill_ms),
        version,
        expires_at,
    ))
}

fn number(
    item: &HashMap<String, AttributeValue>,
    field: &'static str,
) -> Result<u64, RepositoryError> {
    item.get(field)
        .and_then(|value| value.as_n().ok())
        .and_then(|value| value.parse().ok())
        .ok_or(RepositoryError::InvalidItem { field })
}

fn is_missing_table(error: &SdkError<DescribeTableError>) -> bool {
    error
        .as_service_error()
        .is_some_and(DescribeTableError::is_resource_not_found_exception)
}

fn is_table_already_being_created(error: &SdkError<CreateTableError>) -> bool {
    error
        .as_service_error()
        .is_some_and(CreateTableError::is_resource_in_use_exception)
}

fn is_conditional_conflict(error: &SdkError<PutItemError>) -> bool {
    error
        .as_service_error()
        .is_some_and(PutItemError::is_conditional_check_failed_exception)
}
