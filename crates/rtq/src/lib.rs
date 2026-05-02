//! Resource Transition Queue (RTQ) — AMQP-backed message queue for resource transitions.
//!
//! This crate wraps an AMQP 0-9-1 broker (via [`lapin`]) to distribute resource
//! transition messages (create, restore, adopt, destroy, check) across a pool of
//! worker processes. Messages are sharded by resource UID so that all transitions
//! for the same resource land on the same worker, ensuring serial processing.
//!
//! # Message schema
//!
//! Every message is a JSON object with a discriminator field `"type"` set to one
//! of `CREATE`, `RESTORE`, `ADOPT`, `DESTROY`, or `CHECK`. The remaining fields
//! are variant-specific — see [`CreateMessage`], [`RestoreMessage`],
//! [`AdoptMessage`], [`DestroyMessage`], and [`CheckMessage`].
//!
//! ## `inputs` / `desired_inputs`
//!
//! A [`serde_json::Value`] carrying the resource's input record as produced by
//! the SCL evaluator. The exact shape depends on the resource type's plugin
//! contract — it is always a JSON object whose keys are field names and whose
//! values are SCL-encoded primitives.
//!
//! # Dead-letter / retry strategy
//!
//! Messages that cannot be processed are `nack`-ed **without** requeue
//! (`requeue = false`). In production the AMQP broker should be configured with
//! a dead-letter exchange (DLX) and a dead-letter routing key so that
//! permanently unprocessable messages are routed to a dead-letter queue for
//! manual inspection rather than cycling infinitely. See the RabbitMQ
//! documentation on [dead-letter exchanges][dlx] for configuration details.
//!
//! [dlx]: https://www.rabbitmq.com/docs/dlx
//!
//! # AMQP authentication
//!
//! The default URI (`amqp://127.0.0.1:5672/%2f`) uses the guest account which
//! is only permitted to connect from localhost. For production deployments, pass
//! an authenticated URI via [`ClientBuilder::uri`], e.g.
//! `amqp://user:password@broker-host:5672/%2f`. See the [AMQP URI spec][uri]
//! for the full format.
//!
//! [uri]: https://www.rabbitmq.com/docs/uri-spec

use futures_util::StreamExt;
use lapin::{
    BasicProperties, Channel, Connection, ConnectionProperties, ExchangeKind, options::*,
    types::FieldTable,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};
use thiserror::Error;

/// Default AMQP broker URI. Uses the guest account, which is only permitted to
/// connect from localhost. For production, supply an authenticated URI via
/// [`ClientBuilder::uri`].
const DEFAULT_URI: &str = "amqp://127.0.0.1:5672/%2f";

const EXCHANGE_NAME: &str = "rtq.v1";

/// Default number of shards (routing keys) used to distribute work.
const DEFAULT_SHARD_COUNT: u16 = 32;

/// Default per-consumer prefetch count.
const DEFAULT_PREFETCH: u16 = 1;

const DURABLE: bool = true;

/// A tagged union of all resource transition message types.
///
/// Serialized as JSON with a `"type"` discriminator field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Message {
    Create(CreateMessage),
    Restore(RestoreMessage),
    Adopt(AdoptMessage),
    Destroy(DestroyMessage),
    Check(CheckMessage),
}

impl Message {
    fn encode_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    fn decode_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Returns a reference to the [`ResourceRef`] embedded in this message.
    pub fn resource(&self) -> &ResourceRef {
        match self {
            Message::Create(msg) => &msg.resource,
            Message::Restore(msg) => &msg.resource,
            Message::Adopt(msg) => &msg.resource,
            Message::Destroy(msg) => &msg.resource,
            Message::Check(msg) => &msg.resource,
        }
    }

    /// Returns the home region of the repo this dispatch belongs to — i.e.
    /// the region whose RQ should receive the resulting status report. This
    /// may differ from `self.resource().resource_id.region()`, which is the
    /// *target* region where the resource itself is being acted on.
    pub fn home_region(&self) -> &ids::RegionId {
        match self {
            Message::Create(msg) => &msg.home_region,
            Message::Restore(msg) => &msg.home_region,
            Message::Adopt(msg) => &msg.home_region,
            Message::Destroy(msg) => &msg.home_region,
            Message::Check(msg) => &msg.home_region,
        }
    }
}

/// Identifies a resource by its environment and resource ID.
///
/// This struct uses typed [`ids::EnvironmentQid`] and [`ids::ResourceId`]
/// rather than raw strings, ensuring that all identifiers are validated on
/// deserialization and that separator injection is impossible.
///
/// The wire format separates the resource ID into its constituent
/// `region`, `resource_type`, and `resource_name` fields. For the message's
/// primary `resource` the region equals the regional RTQ this message was
/// dispatched to (the consuming RTE can sanity-check); for dependency
/// references the region may name a different region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRef {
    pub environment_qid: ids::EnvironmentQid,
    pub resource_id: ids::ResourceId,
}

impl Serialize for ResourceRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("ResourceRef", 4)?;
        s.serialize_field("environment_qid", &self.environment_qid)?;
        s.serialize_field("region", self.resource_id.region())?;
        s.serialize_field("resource_type", self.resource_id.resource_type())?;
        s.serialize_field("resource_name", self.resource_id.resource_name())?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for ResourceRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            environment_qid: ids::EnvironmentQid,
            region: ids::RegionId,
            resource_type: String,
            resource_name: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        let resource_id = ids::ResourceId::new(raw.region, raw.resource_type, raw.resource_name);
        Ok(ResourceRef {
            environment_qid: raw.environment_qid,
            resource_id,
        })
    }
}

impl ResourceRef {
    /// Returns the environment QID as a string slice.
    pub fn environment_qid_str(&self) -> String {
        self.environment_qid.to_string()
    }

    /// Returns the resource type string (e.g. `"Std/Random.Int"`).
    pub fn resource_type(&self) -> &str {
        self.resource_id.resource_type()
    }

    /// Returns the resource name string (e.g. `"seed"`).
    pub fn resource_name(&self) -> &str {
        self.resource_id.resource_name()
    }

    /// Returns a unique identifier for this resource across the entire system,
    /// used for shard routing.
    fn uid(&self) -> String {
        format!("{}::{}", self.environment_qid, self.resource_id)
    }
}

/// Request to create a new resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateMessage {
    pub resource: ResourceRef,
    pub deployment_id: ids::DeploymentId,
    pub home_region: ids::RegionId,
    pub inputs: Value,
    pub dependencies: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_trace: ids::SourceTrace,
}

/// Request to restore (re-apply) an existing resource with potentially updated inputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreMessage {
    pub resource: ResourceRef,
    pub deployment_id: ids::DeploymentId,
    pub home_region: ids::RegionId,
    pub desired_inputs: Value,
    pub dependencies: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_trace: ids::SourceTrace,
}

/// Request to transfer ownership of a resource from one deployment to another,
/// with potentially updated inputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdoptMessage {
    pub resource: ResourceRef,
    pub from_deployment_id: ids::DeploymentId,
    pub to_deployment_id: ids::DeploymentId,
    pub home_region: ids::RegionId,
    pub desired_inputs: Value,
    pub dependencies: Vec<ResourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_trace: ids::SourceTrace,
}

/// Request to destroy a resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DestroyMessage {
    pub resource: ResourceRef,
    pub deployment_id: ids::DeploymentId,
    pub home_region: ids::RegionId,
}

/// Request to check (verify) the current state of a resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckMessage {
    pub resource: ResourceRef,
    pub deployment_id: ids::DeploymentId,
    pub home_region: ids::RegionId,
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

/// Builder for RTQ publishers and consumers.
///
/// Use [`ClientBuilder::shard_count`] and [`ClientBuilder::prefetch`] to
/// override the default shard and prefetch settings.
pub struct ClientBuilder {
    uri: String,
    shard_count: u16,
    prefetch: u16,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            uri: DEFAULT_URI.to_string(),
            shard_count: DEFAULT_SHARD_COUNT,
            prefetch: DEFAULT_PREFETCH,
        }
    }

    /// Sets the AMQP broker URI. The default connects to localhost without
    /// credentials. For production, embed credentials in the URI, e.g.
    /// `amqp://user:password@host:5672/%2f`.
    pub fn uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = uri.into();
        self
    }

    /// Sets the number of shards (routing keys) used to distribute work across
    /// consumers. All publishers and consumers in a cluster **must** agree on
    /// this value. Defaults to 32.
    pub fn shard_count(mut self, shard_count: u16) -> Self {
        self.shard_count = shard_count;
        self
    }

    /// Sets the AMQP basic.qos prefetch count for consumers. Controls how many
    /// unacknowledged messages a single consumer may hold. Defaults to 1.
    pub fn prefetch(mut self, prefetch: u16) -> Self {
        self.prefetch = prefetch;
        self
    }

    pub async fn build_publisher(self) -> Result<Publisher, ConnectError> {
        let connection = Connection::connect(&self.uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;

        Ok(Publisher {
            _connection: Arc::new(connection),
            publish_channel: channel,
            shard_count: self.shard_count,
        })
    }

    pub async fn build_consumer(self, worker: WorkerConfig) -> Result<Consumer, ConsumerError> {
        worker.validate()?;

        let shard_count = self.shard_count;
        let connection = Connection::connect(&self.uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;
        channel
            .basic_qos(self.prefetch, BasicQosOptions::default())
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
        for shard in 0..shard_count {
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
    shard_count: u16,
}

impl Publisher {
    pub async fn enqueue(&self, message: &Message) -> Result<(), PublishError> {
        let resource_uid = message.resource().uid();
        let shard = shard_for_resource_uid(&resource_uid, self.shard_count);
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

/// Assigns a resource UID to a shard using a deterministic hash.
///
/// Uses `SipHasher` with fixed keys `(0, 0)` to ensure the same resource
/// always maps to the same shard, even across process restarts. This is
/// important because `DefaultHasher` uses a random seed by default, which
/// would break shard assignment across restarts.
#[allow(deprecated)] // SipHasher is stable and deterministic, which is exactly what we need here.
fn shard_for_resource_uid(resource_uid: &str, shard_count: u16) -> u16 {
    let mut hasher = std::hash::SipHasher::new_with_keys(0, 0);
    resource_uid.hash(&mut hasher);
    (hasher.finish() % shard_count as u64) as u16
}

/// Returns the AMQP routing key for a given shard index.
fn shard_routing_key(shard: u16) -> String {
    format!("shard.{shard}")
}

/// Returns the AMQP queue name for a given worker configuration.
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
