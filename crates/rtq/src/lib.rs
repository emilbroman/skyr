use futures_util::StreamExt;
use lapin::{
    BasicProperties, Channel, Connection, ConnectionProperties, ExchangeKind, options::*,
    types::FieldTable,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};
use thiserror::Error;

const DEFAULT_URI: &str = "amqp://127.0.0.1:5672/%2f";
const EXCHANGE_NAME: &str = "rtq.v1";
const SHARD_COUNT: u16 = 32;
const PREFETCH: u16 = 1;
const DURABLE: bool = true;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Message {
    Create(CreateMessage),
    Restore(RestoreMessage),
    Adopt(AdoptMessage),
    Destroy(DestroyMessage),
}

impl Message {
    pub fn encode_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn decode_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    fn resource(&self) -> &ResourceRef {
        match self {
            Message::Create(msg) => &msg.resource,
            Message::Restore(msg) => &msg.resource,
            Message::Adopt(msg) => &msg.resource,
            Message::Destroy(msg) => &msg.resource,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceRef {
    pub namespace: String,
    pub resource_type: String,
    pub resource_id: String,
}

impl ResourceRef {
    pub fn uid(&self) -> String {
        format!(
            "{}:{}:{}",
            self.namespace, self.resource_type, self.resource_id
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateMessage {
    pub resource: ResourceRef,
    pub owner_deployment_id: String,
    pub inputs: Value,
    pub dependencies: Vec<ResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreMessage {
    pub resource: ResourceRef,
    pub owner_deployment_id: String,
    pub desired_inputs: Value,
    pub dependencies: Vec<ResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdoptMessage {
    pub resource: ResourceRef,
    pub from_owner_deployment_id: String,
    pub to_owner_deployment_id: String,
    pub desired_inputs: Value,
    pub dependencies: Vec<ResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DestroyMessage {
    pub resource: ResourceRef,
    pub owner_deployment_id: String,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("worker_count must be at least 1")]
    InvalidWorkerCount,

    #[error("worker_index must be less than worker_count")]
    InvalidWorkerIndex,
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("failed to connect to amqp broker: {0}")]
    Amqp(#[from] lapin::Error),
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("failed to encode message as json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("failed to publish message: {0}")]
    Amqp(#[from] lapin::Error),
}

#[derive(Debug, Error)]
pub enum ConsumerError {
    #[error("invalid worker configuration: {0}")]
    Config(#[from] ConfigError),

    #[error("failed to connect to amqp broker: {0}")]
    Amqp(#[from] lapin::Error),

    #[error("failed to decode message as json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    pub worker_index: u16,
    pub worker_count: u16,
}

impl WorkerConfig {
    pub fn validate(self) -> Result<(), ConfigError> {
        if self.worker_count == 0 {
            return Err(ConfigError::InvalidWorkerCount);
        }
        if self.worker_index >= self.worker_count {
            return Err(ConfigError::InvalidWorkerIndex);
        }
        Ok(())
    }

    fn owns_shard(self, shard: u16) -> bool {
        shard % self.worker_count == self.worker_index
    }
}

pub struct ClientBuilder {
    uri: String,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            uri: DEFAULT_URI.to_string(),
        }
    }

    pub fn uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = uri.into();
        self
    }

    pub async fn build_publisher(self) -> Result<Publisher, ConnectError> {
        let connection = Connection::connect(&self.uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;

        Ok(Publisher {
            _connection: Arc::new(connection),
            publish_channel: channel,
        })
    }

    pub async fn build_consumer(self, worker: WorkerConfig) -> Result<Consumer, ConsumerError> {
        worker.validate()?;

        let connection = Connection::connect(&self.uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;
        channel
            .basic_qos(PREFETCH, BasicQosOptions::default())
            .await?;

        let queue_name = worker_queue_name(worker);
        channel
            .queue_declare(
                &queue_name,
                QueueDeclareOptions {
                    passive: false,
                    durable: DURABLE,
                    exclusive: false,
                    auto_delete: false,
                    nowait: false,
                },
                FieldTable::default(),
            )
            .await?;

        let mut owned_shards = Vec::new();
        for shard in 0..SHARD_COUNT {
            if worker.owns_shard(shard) {
                let routing_key = shard_routing_key(shard);
                channel
                    .queue_bind(
                        &queue_name,
                        EXCHANGE_NAME,
                        &routing_key,
                        QueueBindOptions::default(),
                        FieldTable::default(),
                    )
                    .await?;
                owned_shards.push(shard);
            }
        }

        let consumer_tag = format!(
            "rtq-worker-{}-of-{}",
            worker.worker_index, worker.worker_count
        );
        let inner = channel
            .basic_consume(
                &queue_name,
                &consumer_tag,
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await?;

        Ok(Consumer {
            _connection: Arc::new(connection),
            _channel: channel,
            inner,
            owned_shards,
        })
    }
}

#[derive(Clone)]
pub struct Publisher {
    // Keep the connection alive for the lifetime of channels.
    _connection: Arc<Connection>,
    publish_channel: Channel,
}

impl Publisher {
    pub async fn enqueue(&self, message: &Message) -> Result<(), PublishError> {
        let resource_uid = message.resource().uid();
        let shard = shard_for_resource_uid(&resource_uid, SHARD_COUNT);
        let routing_key = shard_routing_key(shard);
        let payload = message.encode_json()?;

        let confirm = self
            .publish_channel
            .basic_publish(
                EXCHANGE_NAME,
                &routing_key,
                BasicPublishOptions::default(),
                &payload,
                BasicProperties::default().with_content_type("application/json".into()),
            )
            .await?;
        confirm.await?;

        Ok(())
    }
}

pub struct Consumer {
    // Keep connection/channel alive for consumer lifetime.
    _connection: Arc<Connection>,
    // Keep channel alive for consumer lifetime.
    _channel: Channel,
    inner: lapin::Consumer,
    owned_shards: Vec<u16>,
}

impl Consumer {
    pub fn owned_shards(&self) -> &[u16] {
        &self.owned_shards
    }

    pub async fn next(&mut self) -> Result<Option<Delivery>, ConsumerError> {
        let delivery = match self.inner.next().await {
            Some(Ok(delivery)) => delivery,
            Some(Err(err)) => return Err(ConsumerError::Amqp(err)),
            None => return Ok(None),
        };

        let message = Message::decode_json(&delivery.data)?;
        Ok(Some(Delivery { delivery, message }))
    }
}

pub struct Delivery {
    pub message: Message,
    delivery: lapin::message::Delivery,
}

impl Delivery {
    pub fn redelivered(&self) -> bool {
        self.delivery.redelivered
    }

    pub async fn ack(&self) -> Result<(), lapin::Error> {
        self.delivery
            .ack(BasicAckOptions::default())
            .await
            .map(|_| ())
    }

    pub async fn nack(&self, requeue: bool) -> Result<(), lapin::Error> {
        self.delivery
            .nack(BasicNackOptions {
                multiple: false,
                requeue,
            })
            .await
            .map(|_| ())
    }
}

fn shard_for_resource_uid(resource_uid: &str, shard_count: u16) -> u16 {
    let mut hasher = DefaultHasher::new();
    resource_uid.hash(&mut hasher);
    (hasher.finish() % shard_count as u64) as u16
}

fn shard_routing_key(shard: u16) -> String {
    format!("shard.{shard}")
}

fn worker_queue_name(worker: WorkerConfig) -> String {
    format!(
        "{}.worker.{}.of.{}",
        EXCHANGE_NAME, worker.worker_index, worker.worker_count
    )
}

async fn declare_exchange(channel: &Channel) -> Result<(), lapin::Error> {
    channel
        .exchange_declare(
            EXCHANGE_NAME,
            ExchangeKind::Direct,
            ExchangeDeclareOptions {
                passive: false,
                durable: DURABLE,
                auto_delete: false,
                internal: false,
                nowait: false,
            },
            FieldTable::default(),
        )
        .await
}
