//! Notification Queue (NQ) — AMQP-backed message queue for incident notification
//! requests.
//!
//! This crate wraps an AMQP 0-9-1 broker (via [`lapin`]) to carry
//! [`NotificationRequest`] messages from the Reporting Engine (RE, producer) to
//! the Notification Engine (NE, consumer). The NE performs the actual SMTP
//! delivery; this crate is concerned only with reliable transport.
//!
//! Unlike the Reporting Queue (RQ) — which shards by entity QID to preserve
//! per-entity ordering — NQ deliberately has **no sharding** and **no ordering
//! requirement**. Notifications are independent events; multiple NE replicas
//! consume the same single durable queue under plain competing-consumer
//! semantics.
//!
//! # Topology
//!
//! - Direct exchange `nq.v1`, durable.
//! - Single durable queue `nq.v1`, bound to the exchange with the empty
//!   routing key.
//! - Optional dead-letter exchange (DLX) configured via [`ClientBuilder::dead_letter_exchange`]
//!   for messages that exceed retry limits.
//!
//! # Idempotency
//!
//! Delivery is at-least-once. Each [`NotificationRequest`] carries a stable
//! idempotency key derived from `(incident_id, event_type)` — see
//! [`NotificationRequest::idempotency_key`]. NE is responsible for de-duplicating
//! redeliveries against this key.
//!
//! # AMQP authentication
//!
//! The default URI (`amqp://127.0.0.1:5672/%2f`) uses the guest account, which
//! is only permitted to connect from localhost. For production deployments,
//! pass an authenticated URI via [`ClientBuilder::uri`], e.g.
//! `amqp://user:password@broker-host:5672/%2f`.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use lapin::{
    BasicProperties, Channel, Connection, ConnectionProperties, ExchangeKind,
    options::*,
    types::{AMQPValue, FieldTable, ShortString},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default AMQP broker URI. Uses the guest account, which is only permitted to
/// connect from localhost. For production, supply an authenticated URI via
/// [`ClientBuilder::uri`].
const DEFAULT_URI: &str = "amqp://127.0.0.1:5672/%2f";

/// Name of the AMQP exchange used by NQ.
const EXCHANGE_NAME: &str = "nq.v1";

/// Name of the single durable queue used by NQ.
const QUEUE_NAME: &str = "nq.v1";

/// Routing key used between the exchange and the queue. NQ does not shard, so
/// this is a fixed empty string — every publish goes to the single queue.
const ROUTING_KEY: &str = "";

/// Default per-consumer prefetch count.
const DEFAULT_PREFETCH: u16 = 1;

const DURABLE: bool = true;

// ---------------------------------------------------------------------------
// Payload types
// ---------------------------------------------------------------------------

/// Severity category attached to an incident.
///
/// Re-exported from [`rq`] so the entire status-reporting subsystem agrees on
/// one type. The on-the-wire encoding stays SCREAMING_SNAKE_CASE.
pub use rq::IncidentCategory as SeverityCategory;

/// The kind of incident lifecycle event a notification request describes.
///
/// Notifications are emitted on incident **open** and incident **close** only.
/// Mid-incident updates (additional failure reports against an already-open
/// incident) do not produce notification requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationEventType {
    /// An incident has just been opened.
    Opened,
    /// An incident has just been closed.
    Closed,
}

impl NotificationEventType {
    /// Returns the lowercase wire name of this event type.
    fn as_key_str(self) -> &'static str {
        match self {
            NotificationEventType::Opened => "opened",
            NotificationEventType::Closed => "closed",
        }
    }
}

/// A request to send a notification email about an incident state transition.
///
/// The payload is intentionally minimal: it carries only the fields the NE
/// needs to render the email body and resolve recipients (by extracting the
/// owning organization from `entity_qid`). Recipients themselves are *not*
/// snapshotted into this message — NE looks up the org's current membership at
/// send time, so org membership changes between enqueue and dispatch are
/// reflected in the recipient list.
///
/// # Idempotency
///
/// The idempotency key is the pair `(incident_id, event_type)`. It is derived,
/// not stored as a separate field, so the wire schema cannot drift away from
/// it. NE uses [`NotificationRequest::idempotency_key`] to de-duplicate
/// at-least-once redeliveries.
///
/// `entity_qid` is carried as a string rather than a typed enum because NQ's
/// transport plumbing is deliberately entity-agnostic: an entity may be a
/// deployment QID (`OrgId/RepoId::EnvId@DeploymentId.Nonce`) or a resource QID
/// (`OrgId/RepoId::EnvId::ResourceType:ResourceName`), and the queue does not
/// need to discriminate. NE parses the QID when it needs the org for recipient
/// resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationRequest {
    /// Stable, RE-assigned identifier of the incident this notification is
    /// about.
    pub incident_id: String,

    /// Whether this notification is for an incident `Opened` or `Closed` event.
    pub event_type: NotificationEventType,

    /// Stringified QID of the entity the incident is on (a deployment QID or a
    /// resource QID). NE parses this to resolve the owning organization for
    /// recipient lookup.
    pub entity_qid: String,

    /// The classification category of the incident at *open* time. Categories
    /// do not change over an incident's lifetime, so this is the same value for
    /// the corresponding `Opened` and `Closed` notifications.
    pub category: SeverityCategory,

    /// Timestamp at which the incident was opened.
    pub opened_at: DateTime<Utc>,

    /// Timestamp at which the incident was closed. Populated only when
    /// `event_type` is [`NotificationEventType::Closed`]; `None` for `Opened`
    /// notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,

    /// Most recent error message associated with the incident, used in the
    /// email body. May be empty for opens that carry no producer error blurb,
    /// or for closes triggered by a successful heartbeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_message: Option<String>,
}

impl NotificationRequest {
    /// Returns the stable idempotency key for this request, of the form
    /// `"{incident_id}:{opened|closed}"`.
    ///
    /// This key is used by NE to de-duplicate at-least-once redeliveries — the
    /// same `(incident_id, event_type)` pair must produce at most one outgoing
    /// email no matter how many times the message is delivered. NQ also stamps
    /// this key into the AMQP `message_id` property of every published
    /// delivery for broker-side observability.
    pub fn idempotency_key(&self) -> String {
        format!("{}:{}", self.incident_id, self.event_type.as_key_str())
    }

    fn encode_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    fn decode_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("failed to connect to amqp broker: {0}")]
    Amqp(#[from] lapin::Error),
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("failed to encode notification request as json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("failed to publish notification request: {0}")]
    Amqp(#[from] lapin::Error),
}

#[derive(Debug, Error)]
pub enum ConsumerError {
    #[error("failed to connect to amqp broker: {0}")]
    Amqp(#[from] lapin::Error),

    #[error("failed to decode notification request as json: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Client / builder
// ---------------------------------------------------------------------------

/// Builder for NQ publishers and consumers.
///
/// Use [`ClientBuilder::dead_letter_exchange`] to attach a DLX to the queue.
/// All builder calls return `self` so they can be chained.
pub struct ClientBuilder {
    uri: String,
    prefetch: u16,
    dead_letter_exchange: Option<String>,
    dead_letter_routing_key: Option<String>,
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
            prefetch: DEFAULT_PREFETCH,
            dead_letter_exchange: None,
            dead_letter_routing_key: None,
        }
    }

    /// Sets the AMQP broker URI. The default connects to localhost without
    /// credentials. For production, embed credentials in the URI, e.g.
    /// `amqp://user:password@host:5672/%2f`.
    pub fn uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = uri.into();
        self
    }

    /// Sets the AMQP basic.qos prefetch count for consumers. Controls how many
    /// unacknowledged messages a single consumer may hold. Defaults to 1.
    pub fn prefetch(mut self, prefetch: u16) -> Self {
        self.prefetch = prefetch;
        self
    }

    /// Configures a dead-letter exchange (DLX) that the queue will route
    /// `nack`-without-requeue messages and rejected messages to.
    ///
    /// The exchange itself is **not** declared by this crate — operations is
    /// responsible for declaring the DLX (and any bound dead-letter queue)
    /// outside the application. All publishers and consumers in a cluster
    /// **must** agree on whether and how a DLX is configured, because the
    /// queue declaration's `x-dead-letter-exchange` argument is part of the
    /// queue's identity at the broker.
    pub fn dead_letter_exchange(mut self, exchange: impl Into<String>) -> Self {
        self.dead_letter_exchange = Some(exchange.into());
        self
    }

    /// Sets the routing key used when dead-lettering. Only meaningful in
    /// combination with [`ClientBuilder::dead_letter_exchange`]. If unset,
    /// the broker uses the message's original routing key.
    pub fn dead_letter_routing_key(mut self, routing_key: impl Into<String>) -> Self {
        self.dead_letter_routing_key = Some(routing_key.into());
        self
    }

    /// Builds a [`Publisher`].
    ///
    /// Declares the exchange (idempotent) on connection. The single durable
    /// queue is **not** declared here — declaring it requires knowing the DLX
    /// settings, which must agree across the whole cluster. Production
    /// deployments declare the queue once at provisioning time, or via the
    /// consumer side which always declares it.
    pub async fn build_publisher(self) -> Result<Publisher, ConnectError> {
        let connection = Connection::connect(&self.uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;

        Ok(Publisher {
            _connection: Arc::new(connection),
            publish_channel: channel,
        })
    }

    /// Builds a [`Consumer`].
    ///
    /// Declares the exchange and the single durable queue (both idempotent)
    /// and binds the queue to the exchange. If a DLX has been configured via
    /// [`ClientBuilder::dead_letter_exchange`], the queue declaration includes
    /// the corresponding `x-dead-letter-exchange` argument.
    pub async fn build_consumer(self) -> Result<Consumer, ConsumerError> {
        let connection = Connection::connect(&self.uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;
        channel
            .basic_qos(self.prefetch, BasicQosOptions::default())
            .await?;

        let mut queue_args = FieldTable::default();
        if let Some(dlx) = &self.dead_letter_exchange {
            queue_args.insert(
                ShortString::from("x-dead-letter-exchange"),
                AMQPValue::LongString(dlx.as_str().into()),
            );
        }
        if let Some(dlrk) = &self.dead_letter_routing_key {
            queue_args.insert(
                ShortString::from("x-dead-letter-routing-key"),
                AMQPValue::LongString(dlrk.as_str().into()),
            );
        }

        channel
            .queue_declare(
                QUEUE_NAME,
                QueueDeclareOptions {
                    passive: false,
                    durable: DURABLE,
                    exclusive: false,
                    auto_delete: false,
                    nowait: false,
                },
                queue_args,
            )
            .await?;

        channel
            .queue_bind(
                QUEUE_NAME,
                EXCHANGE_NAME,
                ROUTING_KEY,
                QueueBindOptions::default(),
                FieldTable::default(),
            )
            .await?;

        let consumer_tag = format!("nq-consumer-{}", std::process::id());
        let inner = channel
            .basic_consume(
                QUEUE_NAME,
                &consumer_tag,
                BasicConsumeOptions::default(),
                FieldTable::default(),
            )
            .await?;

        Ok(Consumer {
            _connection: Arc::new(connection),
            _channel: channel,
            inner,
        })
    }
}

// ---------------------------------------------------------------------------
// Publisher
// ---------------------------------------------------------------------------

/// Producer end of NQ. Cheap to clone; cloning produces a handle sharing the
/// same underlying connection and channel.
#[derive(Clone)]
pub struct Publisher {
    // Keep the connection alive for the lifetime of channels.
    _connection: Arc<Connection>,
    publish_channel: Channel,
}

impl Publisher {
    /// Publishes a [`NotificationRequest`] to the single NQ queue.
    ///
    /// The request's idempotency key is set as the AMQP `message_id` property
    /// to aid broker-side observability and any future broker-level dedup.
    /// Messages are marked persistent (delivery mode 2) so they survive broker
    /// restarts; the publish is awaited until the broker confirms it.
    pub async fn enqueue(&self, request: &NotificationRequest) -> Result<(), PublishError> {
        let payload = request.encode_json()?;
        let idempotency_key = request.idempotency_key();

        let properties = BasicProperties::default()
            .with_content_type("application/json".into())
            .with_message_id(idempotency_key.into())
            .with_delivery_mode(2);

        let confirm = self
            .publish_channel
            .basic_publish(
                EXCHANGE_NAME,
                ROUTING_KEY,
                BasicPublishOptions::default(),
                &payload,
                properties,
            )
            .await?;
        confirm.await?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Consumer
// ---------------------------------------------------------------------------

/// Consumer end of NQ. Multiple `Consumer`s, possibly on different processes,
/// may pull from the same `nq.v1` queue concurrently — RabbitMQ delivers each
/// message to exactly one of them under competing-consumer semantics.
pub struct Consumer {
    // Keep connection/channel alive for consumer lifetime.
    _connection: Arc<Connection>,
    _channel: Channel,
    inner: lapin::Consumer,
}

impl Consumer {
    /// Awaits the next delivery, decoding it as a [`NotificationRequest`].
    ///
    /// Returns:
    /// - `Ok(Some(delivery))` for a successfully decoded delivery.
    /// - `Ok(None)` if the consumer stream has been cleanly closed.
    /// - `Err(_)` for AMQP transport errors and JSON decode failures. JSON
    ///   decode failures leave the underlying delivery unacknowledged; the
    ///   caller may want to drain such cases via a separate channel — but
    ///   under normal operation, producers and consumers in this crate share a
    ///   schema, so JSON errors indicate a deployment-time mismatch.
    pub async fn next(&mut self) -> Result<Option<Delivery>, ConsumerError> {
        let delivery = match self.inner.next().await {
            Some(Ok(delivery)) => delivery,
            Some(Err(err)) => return Err(ConsumerError::Amqp(err)),
            None => return Ok(None),
        };

        let request = NotificationRequest::decode_json(&delivery.data)?;
        Ok(Some(Delivery { delivery, request }))
    }
}

/// A successfully-decoded notification request along with its underlying AMQP
/// delivery handle. The caller **must** eventually `ack` or `nack` the
/// delivery; dropping it without doing so leaves the broker holding an
/// unacknowledged message until the channel closes.
pub struct Delivery {
    pub request: NotificationRequest,
    delivery: lapin::message::Delivery,
}

impl Delivery {
    /// Returns whether the broker considers this delivery a redelivery — i.e.
    /// the same message has been delivered before. Useful as a hint to the
    /// dedup path; not a substitute for content-level dedup using the
    /// idempotency key.
    pub fn redelivered(&self) -> bool {
        self.delivery.redelivered
    }

    /// Acknowledges this delivery. Call after the notification has been sent
    /// (or deduped against a prior send) and the outcome is durable.
    pub async fn ack(&self) -> Result<(), lapin::Error> {
        self.delivery
            .ack(BasicAckOptions::default())
            .await
            .map(|_| ())
    }

    /// Negatively acknowledges this delivery. With `requeue = true`, the
    /// broker returns the message to the queue for redelivery. With
    /// `requeue = false`, the message is dropped (or routed to the DLX, if
    /// one was configured at queue declaration time).
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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_open_request() -> NotificationRequest {
        NotificationRequest {
            incident_id: "01HZX9P5K2JN7YQVJ3Q6T4ZB8N".to_string(),
            event_type: NotificationEventType::Opened,
            entity_qid:
                "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
                    .to_string(),
            category: SeverityCategory::Crash,
            opened_at: DateTime::parse_from_rfc3339("2026-04-25T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            closed_at: None,
            last_error_message: Some("plugin returned EOF".to_string()),
        }
    }

    fn sample_close_request() -> NotificationRequest {
        NotificationRequest {
            incident_id: "01HZX9P5K2JN7YQVJ3Q6T4ZB8N".to_string(),
            event_type: NotificationEventType::Closed,
            entity_qid:
                "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
                    .to_string(),
            category: SeverityCategory::Crash,
            opened_at: DateTime::parse_from_rfc3339("2026-04-25T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            closed_at: Some(
                DateTime::parse_from_rfc3339("2026-04-25T12:05:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
            last_error_message: None,
        }
    }

    #[test]
    fn idempotency_key_distinguishes_open_and_close() {
        let open = sample_open_request();
        let close = sample_close_request();
        assert_ne!(open.idempotency_key(), close.idempotency_key());
        assert_eq!(open.idempotency_key(), "01HZX9P5K2JN7YQVJ3Q6T4ZB8N:opened");
        assert_eq!(close.idempotency_key(), "01HZX9P5K2JN7YQVJ3Q6T4ZB8N:closed");
    }

    #[test]
    fn idempotency_key_is_stable_under_clone_and_reserialize() {
        let req = sample_open_request();
        let key_a = req.idempotency_key();
        let json = serde_json::to_vec(&req).unwrap();
        let req2: NotificationRequest = serde_json::from_slice(&json).unwrap();
        assert_eq!(key_a, req2.idempotency_key());
    }

    #[test]
    fn open_request_round_trips_through_json() {
        let req = sample_open_request();
        let bytes = req.encode_json().unwrap();
        let decoded = NotificationRequest::decode_json(&bytes).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn close_request_round_trips_through_json() {
        let req = sample_close_request();
        let bytes = req.encode_json().unwrap();
        let decoded = NotificationRequest::decode_json(&bytes).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn closed_at_is_omitted_for_open_events() {
        let open = sample_open_request();
        let v: serde_json::Value = serde_json::from_slice(&open.encode_json().unwrap()).unwrap();
        let obj = v.as_object().unwrap();
        assert!(
            !obj.contains_key("closed_at"),
            "open requests must not serialize closed_at: {obj:?}",
        );
    }

    #[test]
    fn last_error_message_is_omitted_when_none() {
        let close = sample_close_request();
        let v: serde_json::Value = serde_json::from_slice(&close.encode_json().unwrap()).unwrap();
        let obj = v.as_object().unwrap();
        assert!(
            !obj.contains_key("last_error_message"),
            "None last_error_message must be omitted on the wire: {obj:?}",
        );
    }

    #[test]
    fn event_type_uses_screaming_snake_case_on_the_wire() {
        let open = sample_open_request();
        let v: serde_json::Value = serde_json::from_slice(&open.encode_json().unwrap()).unwrap();
        assert_eq!(v["event_type"], serde_json::json!("OPENED"));

        let close = sample_close_request();
        let v: serde_json::Value = serde_json::from_slice(&close.encode_json().unwrap()).unwrap();
        assert_eq!(v["event_type"], serde_json::json!("CLOSED"));
    }

    #[test]
    fn category_uses_screaming_snake_case_on_the_wire() {
        let mut req = sample_open_request();
        req.category = SeverityCategory::SystemError;
        let v: serde_json::Value = serde_json::from_slice(&req.encode_json().unwrap()).unwrap();
        assert_eq!(v["category"], serde_json::json!("SYSTEM_ERROR"));

        req.category = SeverityCategory::BadConfiguration;
        let v: serde_json::Value = serde_json::from_slice(&req.encode_json().unwrap()).unwrap();
        assert_eq!(v["category"], serde_json::json!("BAD_CONFIGURATION"));

        req.category = SeverityCategory::CannotProgress;
        let v: serde_json::Value = serde_json::from_slice(&req.encode_json().unwrap()).unwrap();
        assert_eq!(v["category"], serde_json::json!("CANNOT_PROGRESS"));

        req.category = SeverityCategory::InconsistentState;
        let v: serde_json::Value = serde_json::from_slice(&req.encode_json().unwrap()).unwrap();
        assert_eq!(v["category"], serde_json::json!("INCONSISTENT_STATE"));

        req.category = SeverityCategory::Crash;
        let v: serde_json::Value = serde_json::from_slice(&req.encode_json().unwrap()).unwrap();
        assert_eq!(v["category"], serde_json::json!("CRASH"));
    }
}
