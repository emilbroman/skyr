//! Reporting Queue (RQ) — AMQP-backed message queue for status reports.
//!
//! This crate wraps an AMQP 0-9-1 broker (via [`lapin`]) to distribute
//! [`Report`] messages from the Skyr engines (DE, RTE) to the Reporting Engine
//! (RE). Reports are sharded by entity QID so that all reports for the same
//! entity land on the same RE worker, ensuring serial per-entity processing.
//!
//! # Report schema
//!
//! Every report is a JSON object containing:
//!
//! - The entity QID — either a deployment QID or a resource QID — encoded as a
//!   tagged enum with a `"kind"` discriminator (`"deployment"` or `"resource"`)
//!   and a `"qid"` string.
//! - A UTC timestamp.
//! - An [`Outcome`]: `success` or `failure { category, error_message }`. The
//!   five severity categories are listed in [`IncidentCategory`].
//! - A [`Metrics`] envelope with a known `wall_time_ms` field and an
//!   open-ended `extra` map for forward-compatible additions.
//! - An [`EntityExtension`] — a typed enum carrying entity-scoped fields. The
//!   transport layer never pattern-matches on this; only entity-aware
//!   consumer logic (in `re`) does.
//!
//! # Topology
//!
//! Mirrors RTQ:
//!
//! - Direct exchange `rq.v1`, durable.
//! - Routing keys `shard.0` … `shard.{N-1}` (default `N = 32`).
//! - Per-worker queues `rq.v1.worker.{i}.of.{count}`, durable, each bound to a
//!   contiguous range of shards.
//! - The shard for a report is determined by a deterministic SipHash of the
//!   entity QID's string form — independent of process restarts.
//!
//! # Dead-letter / retry strategy
//!
//! Messages that cannot be processed are `nack`-ed **without** requeue
//! (`requeue = false`). In production the AMQP broker should be configured
//! with a dead-letter exchange (DLX) and a dead-letter routing key so that
//! permanently unprocessable messages are routed to a dead-letter queue for
//! manual inspection rather than cycling infinitely. See the RabbitMQ
//! documentation on [dead-letter exchanges][dlx] for configuration details.
//!
//! [dlx]: https://www.rabbitmq.com/docs/dlx
//!
//! # AMQP authentication
//!
//! The default URI (`amqp://127.0.0.1:5672/%2f`) uses the guest account which
//! is only permitted to connect from localhost. For production deployments,
//! pass an authenticated URI via [`ClientBuilder::uri`], e.g.
//! `amqp://user:password@broker-host:5672/%2f`. See the [AMQP URI spec][uri]
//! for the full format.
//!
//! [uri]: https://www.rabbitmq.com/docs/uri-spec

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use lapin::{
    BasicProperties, Channel, Connection, ConnectionProperties, ExchangeKind, options::*,
    types::FieldTable,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    hash::{Hash, Hasher},
    sync::Arc,
};
use thiserror::Error;
use tokio::sync::Mutex;

/// Default AMQP broker URI. Uses the guest account, which is only permitted to
/// connect from localhost. For production, supply an authenticated URI via
/// [`ClientBuilder::uri`].
const DEFAULT_URI: &str = "amqp://127.0.0.1:5672/%2f";

const EXCHANGE_NAME: &str = "rq.v1";

/// Default number of shards (routing keys) used to distribute work.
const DEFAULT_SHARD_COUNT: u16 = 32;

/// Default per-consumer prefetch count.
const DEFAULT_PREFETCH: u16 = 1;

const DURABLE: bool = true;

// ---------------------------------------------------------------------------
// Severity category
// ---------------------------------------------------------------------------

/// Producer-assigned classification of a failure report.
///
/// These are deliberately *consequence-oriented* — they describe the
/// user-visible impact of the failure, not where the failure came from. The
/// RE applies per-category threshold rules to decide whether a sequence of
/// failures constitutes an incident.
///
/// The variants are listed in roughly increasing severity. The derived
/// `PartialOrd` / `Ord` follow declaration order so callers can compute
/// "worst category" with `cmp::max` if they wish.
///
/// This enum is the source of truth for the category set across the
/// status-reporting subsystem. Downstream crates (`sdb`, `re`) re-export or
/// import it from here rather than defining their own copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IncidentCategory {
    /// The system is working correctly and is refusing to roll out
    /// configuration it has determined to be invalid (HTTP 4XX analogue).
    BadConfiguration,
    /// The entity itself is stable, but a derived/dependent configuration
    /// could not be applied.
    CannotProgress,
    /// The configuration DAG has drifted from reality and reconciliation
    /// failed due to an irreconcilable inconsistency.
    InconsistentState,
    /// A failure in Skyr's own infrastructure — broker, DB, plugin host, etc.
    /// (HTTP 5XX analogue.)
    SystemError,
    /// The entity is not behaving as intended, resulting in user-visible
    /// downtime.
    Crash,
}

impl IncidentCategory {
    /// All five categories in declaration (severity) order, least-severe first.
    pub const ALL: [IncidentCategory; 5] = [
        IncidentCategory::BadConfiguration,
        IncidentCategory::CannotProgress,
        IncidentCategory::InconsistentState,
        IncidentCategory::SystemError,
        IncidentCategory::Crash,
    ];

    /// The canonical SCREAMING_SNAKE_CASE name used on the wire and in the SDB.
    pub fn as_str(self) -> &'static str {
        match self {
            IncidentCategory::BadConfiguration => "BAD_CONFIGURATION",
            IncidentCategory::CannotProgress => "CANNOT_PROGRESS",
            IncidentCategory::InconsistentState => "INCONSISTENT_STATE",
            IncidentCategory::SystemError => "SYSTEM_ERROR",
            IncidentCategory::Crash => "CRASH",
        }
    }
}

impl std::fmt::Display for IncidentCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string cannot be parsed as an [`IncidentCategory`].
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid incident category: {0:?}")]
pub struct InvalidCategory(pub String);

impl std::str::FromStr for IncidentCategory {
    type Err = InvalidCategory;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "BAD_CONFIGURATION" => Ok(IncidentCategory::BadConfiguration),
            "CANNOT_PROGRESS" => Ok(IncidentCategory::CannotProgress),
            "INCONSISTENT_STATE" => Ok(IncidentCategory::InconsistentState),
            "SYSTEM_ERROR" => Ok(IncidentCategory::SystemError),
            "CRASH" => Ok(IncidentCategory::Crash),
            other => Err(InvalidCategory(other.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Entity QID
// ---------------------------------------------------------------------------

/// The entity a report is about.
///
/// v1 tracks two entity types: deployments (via [`ids::DeploymentQid`]) and
/// resources (via [`ids::ResourceQid`]). Higher-level rollups (environment,
/// repo, organization) are computed at the API layer and are not first-class
/// entities here.
///
/// The wire format uses an explicit `kind` discriminator and a `qid` string,
/// keeping the JSON stable even if the typed `ids::*` types add internal
/// fields later.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntityQid {
    Deployment(ids::DeploymentQid),
    Resource(ids::ResourceQid),
}

impl EntityQid {
    /// Returns the canonical string form used for shard routing.
    pub fn shard_key(&self) -> String {
        match self {
            EntityQid::Deployment(qid) => qid.to_string(),
            EntityQid::Resource(qid) => qid.to_string(),
        }
    }

    /// Returns the canonical wire string for this QID.
    pub fn as_string(&self) -> String {
        self.shard_key()
    }
}

impl Serialize for EntityQid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("EntityQid", 2)?;
        match self {
            EntityQid::Deployment(qid) => {
                s.serialize_field("kind", "deployment")?;
                s.serialize_field("qid", &qid.to_string())?;
            }
            EntityQid::Resource(qid) => {
                s.serialize_field("kind", "resource")?;
                s.serialize_field("qid", &qid.to_string())?;
            }
        }
        s.end()
    }
}

impl<'de> Deserialize<'de> for EntityQid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            kind: String,
            qid: String,
        }
        let raw = Raw::deserialize(deserializer)?;
        match raw.kind.as_str() {
            "deployment" => {
                let qid: ids::DeploymentQid = raw.qid.parse().map_err(serde::de::Error::custom)?;
                Ok(EntityQid::Deployment(qid))
            }
            "resource" => {
                let qid: ids::ResourceQid = raw.qid.parse().map_err(serde::de::Error::custom)?;
                Ok(EntityQid::Resource(qid))
            }
            other => Err(serde::de::Error::custom(format!(
                "unknown entity kind: {other}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// Result of an operation a producer is reporting on.
///
/// Encoded as a tagged JSON enum with a `"status"` discriminator. Successful
/// reports carry no category or error message — those are only present on
/// failure, enforced by this type's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Outcome {
    Success,
    Failure {
        category: IncidentCategory,
        error_message: String,
    },
}

impl Outcome {
    /// Returns `true` if this outcome represents success.
    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Success)
    }

    /// Returns the failure category, or `None` for successful outcomes.
    pub fn category(&self) -> Option<IncidentCategory> {
        match self {
            Outcome::Success => None,
            Outcome::Failure { category, .. } => Some(*category),
        }
    }

    /// Returns the error message, or `None` for successful outcomes.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Outcome::Success => None,
            Outcome::Failure { error_message, .. } => Some(error_message.as_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Diagnostic metrics attached to a report.
///
/// v1 producers only fill [`Metrics::wall_time_ms`]; the `extra` map is
/// reserved for future additions (e.g. plugin-reported resource metrics over
/// RTP) and is forward-compatible — older consumers will ignore unknown
/// fields, and new producers can add fields without a schema break.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    /// Wall-clock time spent processing the operation, in milliseconds.
    pub wall_time_ms: u64,

    /// Open-ended map for future metric additions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

impl Metrics {
    /// Convenience constructor for the v1 case (wall time only).
    pub fn wall_time(wall_time_ms: u64) -> Self {
        Self {
            wall_time_ms,
            extra: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Entity-scoped extensions
// ---------------------------------------------------------------------------

/// Operational state of a deployment, as observed by its producer at report
/// time.
///
/// Used by the RE watchdog to decide what reporting cadence to expect for a
/// deployment in this state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeploymentOperationalState {
    /// The deployment is the desired (latest) revision and the DE is actively
    /// reconciling it. Highest expected reporting cadence.
    Desired,
    /// The deployment is no longer the desired revision but is still being
    /// torn down. Lower expected cadence.
    Undesired,
    /// The deployment has been superseded but its resources have not yet been
    /// migrated away. Lowest expected cadence.
    Lingering,
    /// The deployment has reached the DOWN terminal state. Reports in this
    /// state typically also carry the `terminal` flag.
    Down,
}

/// Operational state of a resource, as observed by its producer at report
/// time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceOperationalState {
    /// The resource has not yet been created.
    Pending,
    /// The resource is created and currently meeting its desired state.
    Live,
    /// The resource has been destroyed. Reports in this state typically also
    /// carry the `terminal` flag.
    Destroyed,
}

/// Deployment-scoped extension fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeploymentExtension {
    /// The deployment's current operational state at report time. Cached by
    /// the RE in the per-entity status summary so the watchdog knows what
    /// cadence to expect.
    pub operational_state: DeploymentOperationalState,

    /// `true` if this is the deployment's terminal report (the entity has
    /// reached DOWN and will not be reported on again). The RE deletes the
    /// per-entity status summary on receiving a terminal report.
    #[serde(default)]
    pub terminal: bool,
}

/// Resource-scoped extension fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceExtension {
    /// The resource's current operational state at report time.
    pub operational_state: ResourceOperationalState,

    /// `true` if this is the resource's terminal report (the resource has
    /// been destroyed).
    #[serde(default)]
    pub terminal: bool,

    /// `true` if the producer has marked this resource as volatile (its
    /// plugin has the `sclc::Marker::Volatile` marker set). Non-volatile
    /// resources do not receive periodic Check messages, so the RE watchdog
    /// must not expect heartbeats for them while they sit in `Live`.
    ///
    /// Defaults to `false` on the wire so older producers and reports from
    /// failure paths that cannot determine volatility decode safely. An
    /// inaccurate `false` is conservative: the watchdog will not track that
    /// entry, and the next successful report flips the flag if the resource
    /// really is volatile.
    #[serde(default)]
    pub volatile: bool,
}

/// Typed, entity-scoped extension carried alongside the common report base.
///
/// **The transport layer must not pattern-match on this enum.** Shard
/// routing, publishing, and consumption all operate on the report's common
/// base fields. Only entity-aware consumer logic (in `re`) inspects the
/// extension variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EntityExtension {
    Deployment(DeploymentExtension),
    Resource(ResourceExtension),
}

impl EntityExtension {
    /// Returns whether this extension's terminal flag is set, without
    /// requiring the caller to pattern-match on the variant.
    ///
    /// This is the only field of the extension that the entity-agnostic
    /// pipeline looks at — the RE uses it to know when to delete a per-entity
    /// status summary row. Reading it through this accessor keeps the
    /// transport layer from pattern-matching on the variant.
    pub fn is_terminal(&self) -> bool {
        match self {
            EntityExtension::Deployment(ext) => ext.terminal,
            EntityExtension::Resource(ext) => ext.terminal,
        }
    }

    /// Returns whether the entity is a volatile resource. Always `false` for
    /// deployments — volatility is a per-resource property reflecting whether
    /// the resource's plugin has set the `Volatile` marker.
    pub fn is_volatile(&self) -> bool {
        match self {
            EntityExtension::Deployment(_) => false,
            EntityExtension::Resource(ext) => ext.volatile,
        }
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// A single report from a Skyr engine to the RE.
///
/// All producers (DE, RTE) emit a `Report` for every operation, success or
/// failure. The common base — `entity_qid`, `timestamp`, `outcome`, `metrics`
/// — is entity-agnostic and is what the transport and the bulk of the RE
/// pipeline reasons about. The `extension` carries entity-scoped fields and
/// is interpreted only by entity-aware code paths in the RE.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The entity this report is about.
    pub entity_qid: EntityQid,

    /// Wall-clock timestamp at which the producer finished the operation.
    pub timestamp: DateTime<Utc>,

    /// Whether the operation succeeded, and — on failure — its category and
    /// error message.
    pub outcome: Outcome,

    /// Diagnostic metrics. v1 producers fill `wall_time_ms` only.
    #[serde(default)]
    pub metrics: Metrics,

    /// Entity-scoped extension fields.
    pub extension: EntityExtension,
}

impl Report {
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
pub enum ConfigError {
    #[error("worker_count must be at least 1")]
    InvalidWorkerCount,

    #[error("worker_index must be less than worker_count")]
    InvalidWorkerIndex,
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("failed to encode report as json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("failed to publish report: {0}")]
    Amqp(#[from] lapin::Error),
}

#[derive(Debug, Error)]
pub enum ConsumerError {
    #[error("invalid worker configuration: {0}")]
    Config(#[from] ConfigError),

    #[error("failed to connect to amqp broker: {0}")]
    Amqp(#[from] lapin::Error),

    #[error("failed to decode report as json: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Worker config
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Client builder, publisher, consumer
// ---------------------------------------------------------------------------

/// Builder for RQ consumers.
///
/// Publishers are constructed directly via [`Publisher::new`] /
/// [`Publisher::with_shard_count`] — they manage one connection per region
/// internally and do not need a URI.
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

    /// Sets the number of shards (routing keys) used to distribute work
    /// across consumers. All publishers and consumers in a cluster **must**
    /// agree on this value. Defaults to 32.
    pub fn shard_count(mut self, shard_count: u16) -> Self {
        self.shard_count = shard_count;
        self
    }

    /// Sets the AMQP basic.qos prefetch count for consumers. Controls how
    /// many unacknowledged messages a single consumer may hold. Defaults to 1.
    pub fn prefetch(mut self, prefetch: u16) -> Self {
        self.prefetch = prefetch;
        self
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
            "rq-worker-{}-of-{}",
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

/// Cross-region RQ publisher.
///
/// A single [`Publisher`] addresses every region's RQ. AMQP connections are
/// established lazily on first publish to a given region and reused
/// thereafter; the per-region URI is derived from the configured
/// [`ids::Domain`] via [`ids::service_address`]. Status reports are routed
/// to the *home* region's RQ (the home region is whichever region the repo
/// is pinned to), so an RTE running in `tokyo` for a repo homed in `paris`
/// publishes to `rq.paris.int.<domain>`.
///
/// In a single-region deployment exactly one connection is ever opened (to
/// the local region's broker), giving the same operational profile as a
/// pre-multi-region single-broker publisher.
#[derive(Clone)]
pub struct Publisher {
    domain: ids::Domain,
    shard_count: u16,
    connections: Arc<Mutex<HashMap<ids::RegionId, RegionalConnection>>>,
}

#[derive(Clone)]
struct RegionalConnection {
    // Keep the connection alive for the lifetime of channels.
    _connection: Arc<Connection>,
    channel: Channel,
}

impl Publisher {
    /// Construct a publisher addressing every region under `domain`. The
    /// shard count must match every consumer's shard count — defaults to
    /// [`DEFAULT_SHARD_COUNT`].
    pub fn new(domain: ids::Domain) -> Self {
        Self::with_shard_count(domain, DEFAULT_SHARD_COUNT)
    }

    pub fn with_shard_count(domain: ids::Domain, shard_count: u16) -> Self {
        Self {
            domain,
            shard_count,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Publish `report` onto the RQ in `region`.
    ///
    /// The shard is selected by hashing the report's entity QID string —
    /// this is the only piece of the report the transport layer reads.
    pub async fn enqueue(
        &self,
        region: &ids::RegionId,
        report: &Report,
    ) -> Result<(), PublishError> {
        let channel = self.channel_for(region).await?;
        let shard_key = report.entity_qid.shard_key();
        let shard = shard_for_entity_key(&shard_key, self.shard_count);
        let routing_key = shard_routing_key(shard);
        let payload = report.encode_json()?;

        let confirm = channel
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

    async fn channel_for(&self, region: &ids::RegionId) -> Result<Channel, PublishError> {
        let mut connections = self.connections.lock().await;
        if let Some(rc) = connections.get(region) {
            return Ok(rc.channel.clone());
        }
        let uri = format!(
            "amqp://{}:5672/%2f",
            ids::service_address("rq", region, &self.domain)
        );
        let connection = Connection::connect(&uri, ConnectionProperties::default()).await?;
        let channel = connection.create_channel().await?;
        declare_exchange(&channel).await?;
        let returned = channel.clone();
        connections.insert(
            region.clone(),
            RegionalConnection {
                _connection: Arc::new(connection),
                channel,
            },
        );
        Ok(returned)
    }
}

pub struct Consumer {
    // Keep connection/channel alive for consumer lifetime.
    _connection: Arc<Connection>,
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

        let report = Report::decode_json(&delivery.data)?;
        Ok(Some(Delivery { delivery, report }))
    }
}

pub struct Delivery {
    pub report: Report,
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

// ---------------------------------------------------------------------------
// Topology helpers
// ---------------------------------------------------------------------------

/// Assigns an entity-key string to a shard using a deterministic hash.
///
/// Uses `SipHasher` with fixed keys `(0, 0)` to ensure the same entity always
/// maps to the same shard, even across process restarts. This mirrors RTQ.
#[allow(deprecated)] // SipHasher is stable and deterministic, which is exactly what we need here.
fn shard_for_entity_key(entity_key: &str, shard_count: u16) -> u16 {
    let mut hasher = std::hash::SipHasher::new_with_keys(0, 0);
    entity_key.hash(&mut hasher);
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_deployment_qid() -> ids::DeploymentQid {
        "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268.a1b2c3d4e5f60718"
            .parse()
            .unwrap()
    }

    fn sample_resource_qid() -> ids::ResourceQid {
        "MyOrg/MyRepo::main::stockholm:Std/Random.Int:seed"
            .parse()
            .unwrap()
    }

    fn sample_timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn entity_qid_deployment_round_trip() {
        let entity = EntityQid::Deployment(sample_deployment_qid());
        let json = serde_json::to_string(&entity).unwrap();
        let back: EntityQid = serde_json::from_str(&json).unwrap();
        assert_eq!(entity, back);
        assert!(json.contains("\"kind\":\"deployment\""));
    }

    #[test]
    fn entity_qid_resource_round_trip() {
        let entity = EntityQid::Resource(sample_resource_qid());
        let json = serde_json::to_string(&entity).unwrap();
        let back: EntityQid = serde_json::from_str(&json).unwrap();
        assert_eq!(entity, back);
        assert!(json.contains("\"kind\":\"resource\""));
    }

    #[test]
    fn incident_category_round_trip() {
        let categories = [
            IncidentCategory::SystemError,
            IncidentCategory::BadConfiguration,
            IncidentCategory::CannotProgress,
            IncidentCategory::InconsistentState,
            IncidentCategory::Crash,
        ];
        for c in categories {
            let json = serde_json::to_string(&c).unwrap();
            let back: IncidentCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn incident_category_ordering_follows_severity() {
        assert!(IncidentCategory::BadConfiguration < IncidentCategory::Crash);
        assert!(IncidentCategory::CannotProgress < IncidentCategory::Crash);
        assert!(IncidentCategory::InconsistentState < IncidentCategory::Crash);
        assert!(IncidentCategory::SystemError < IncidentCategory::Crash);
    }

    #[test]
    fn outcome_success_has_no_category() {
        let outcome = Outcome::Success;
        assert!(outcome.is_success());
        assert_eq!(outcome.category(), None);
        assert_eq!(outcome.error_message(), None);
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(!json.contains("category"));
        assert!(!json.contains("error_message"));
    }

    #[test]
    fn outcome_failure_carries_classification() {
        let outcome = Outcome::Failure {
            category: IncidentCategory::Crash,
            error_message: "boom".to_string(),
        };
        assert!(!outcome.is_success());
        assert_eq!(outcome.category(), Some(IncidentCategory::Crash));
        assert_eq!(outcome.error_message(), Some("boom"));
        let back: Outcome =
            serde_json::from_str(&serde_json::to_string(&outcome).unwrap()).unwrap();
        assert_eq!(outcome, back);
    }

    #[test]
    fn metrics_serialise_extra_omitted_when_empty() {
        let metrics = Metrics::wall_time(42);
        let json = serde_json::to_string(&metrics).unwrap();
        assert!(json.contains("\"wall_time_ms\":42"));
        assert!(!json.contains("extra"));
    }

    #[test]
    fn metrics_round_trip_with_extra() {
        let mut metrics = Metrics::wall_time(7);
        metrics.extra.insert(
            "memory_bytes".to_string(),
            Value::Number(serde_json::Number::from(1024u64)),
        );
        let json = serde_json::to_string(&metrics).unwrap();
        let back: Metrics = serde_json::from_str(&json).unwrap();
        assert_eq!(metrics, back);
    }

    #[test]
    fn metrics_default_when_field_absent() {
        // Producers that never write `metrics` should still decode (forward
        // compat from older test fixtures, defensive on the wire format).
        let report = Report {
            entity_qid: EntityQid::Deployment(sample_deployment_qid()),
            timestamp: sample_timestamp(),
            outcome: Outcome::Success,
            metrics: Metrics::default(),
            extension: EntityExtension::Deployment(DeploymentExtension {
                operational_state: DeploymentOperationalState::Desired,
                terminal: false,
            }),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
    }

    #[test]
    fn deployment_report_round_trip() {
        let report = Report {
            entity_qid: EntityQid::Deployment(sample_deployment_qid()),
            timestamp: sample_timestamp(),
            outcome: Outcome::Failure {
                category: IncidentCategory::SystemError,
                error_message: "broker unreachable".to_string(),
            },
            metrics: Metrics::wall_time(1234),
            extension: EntityExtension::Deployment(DeploymentExtension {
                operational_state: DeploymentOperationalState::Desired,
                terminal: false,
            }),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
    }

    #[test]
    fn resource_report_round_trip_with_terminal_flag() {
        let report = Report {
            entity_qid: EntityQid::Resource(sample_resource_qid()),
            timestamp: sample_timestamp(),
            outcome: Outcome::Success,
            metrics: Metrics::wall_time(5),
            extension: EntityExtension::Resource(ResourceExtension {
                operational_state: ResourceOperationalState::Destroyed,
                terminal: true,
                volatile: false,
            }),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
        assert!(report.extension.is_terminal());
    }

    #[test]
    fn extension_terminal_default_is_false() {
        // Older producers may omit `terminal`; it should default to false.
        let json = r#"{
            "kind": "DEPLOYMENT",
            "operational_state": "DESIRED"
        }"#;
        let ext: EntityExtension = serde_json::from_str(json).unwrap();
        assert!(!ext.is_terminal());
    }

    #[test]
    fn resource_extension_volatile_default_is_false() {
        // Producers that pre-date the volatile flag should still decode, with
        // the flag defaulting to false. The watchdog interprets that as "do
        // not expect heartbeats", which matches the safe behavior.
        let json = r#"{
            "kind": "RESOURCE",
            "operational_state": "LIVE"
        }"#;
        let ext: EntityExtension = serde_json::from_str(json).unwrap();
        assert!(!ext.is_volatile());
    }

    #[test]
    fn deployment_extension_is_volatile_is_always_false() {
        let ext = EntityExtension::Deployment(DeploymentExtension {
            operational_state: DeploymentOperationalState::Desired,
            terminal: false,
        });
        assert!(!ext.is_volatile());
    }

    #[test]
    fn resource_extension_volatile_round_trips() {
        let report = Report {
            entity_qid: EntityQid::Resource(sample_resource_qid()),
            timestamp: sample_timestamp(),
            outcome: Outcome::Success,
            metrics: Metrics::wall_time(5),
            extension: EntityExtension::Resource(ResourceExtension {
                operational_state: ResourceOperationalState::Live,
                terminal: false,
                volatile: true,
            }),
        };
        let json = serde_json::to_string(&report).unwrap();
        let back: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(report, back);
        assert!(back.extension.is_volatile());
    }

    #[test]
    fn shard_assignment_is_deterministic_and_in_range() {
        let key = "MyOrg/MyRepo::main::stockholm:Std/Random.Int:seed";
        let a = shard_for_entity_key(key, 32);
        let b = shard_for_entity_key(key, 32);
        assert_eq!(a, b);
        assert!(a < 32);
    }

    #[test]
    fn shard_assignment_distributes_across_shards() {
        // Sanity check: across many distinct keys we should see at least a
        // handful of distinct shards. Exact distribution is not required.
        use std::collections::HashSet;
        let mut shards = HashSet::new();
        for i in 0..256 {
            let key = format!("MyOrg/MyRepo::env{i}");
            shards.insert(shard_for_entity_key(&key, 32));
        }
        assert!(
            shards.len() > 8,
            "expected good shard distribution, got {}",
            shards.len()
        );
    }

    #[test]
    fn worker_config_validates() {
        assert!(
            WorkerConfig {
                worker_index: 0,
                worker_count: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            WorkerConfig {
                worker_index: 4,
                worker_count: 4
            }
            .validate()
            .is_err()
        );
        assert!(
            WorkerConfig {
                worker_index: 0,
                worker_count: 1
            }
            .validate()
            .is_ok()
        );
        assert!(
            WorkerConfig {
                worker_index: 3,
                worker_count: 4
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn worker_owns_shards_is_partition() {
        // Every shard must be owned by exactly one worker, regardless of
        // worker_count.
        for worker_count in 1u16..=8 {
            for shard in 0u16..32 {
                let owners: Vec<u16> = (0..worker_count)
                    .filter(|i| {
                        WorkerConfig {
                            worker_index: *i,
                            worker_count,
                        }
                        .owns_shard(shard)
                    })
                    .collect();
                assert_eq!(
                    owners.len(),
                    1,
                    "shard {shard} with worker_count {worker_count} had owners {owners:?}"
                );
            }
        }
    }

    #[test]
    fn topology_naming_matches_spec() {
        assert_eq!(EXCHANGE_NAME, "rq.v1");
        assert_eq!(shard_routing_key(0), "shard.0");
        assert_eq!(shard_routing_key(31), "shard.31");
        assert_eq!(
            worker_queue_name(WorkerConfig {
                worker_index: 2,
                worker_count: 4
            }),
            "rq.v1.worker.2.of.4"
        );
    }

    #[test]
    fn entity_qid_shard_key_is_canonical_string() {
        let dep = sample_deployment_qid();
        let entity = EntityQid::Deployment(dep.clone());
        assert_eq!(entity.shard_key(), dep.to_string());

        let res = sample_resource_qid();
        let entity = EntityQid::Resource(res.clone());
        assert_eq!(entity.shard_key(), res.to_string());
    }
}
