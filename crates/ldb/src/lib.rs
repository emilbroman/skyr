use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::sync::Mutex;

use async_stream::try_stream;
use base64::Engine as _;
use chrono::Utc;
use futures_util::stream::Stream;
use rskafka::{
    client::{
        ClientBuilder as RskafkaClientBuilder,
        error::{Error as KafkaError, ProtocolError},
        partition::{Compression, OffsetAt, UnknownTopicHandling},
    },
    record::{Record, RecordAndOffset},
};
use thiserror::Error;

const DEFAULT_BROKERS: &str = "127.0.0.1:9092";
const TOPIC_PREFIX: &str = "dl-";
const FETCH_MAX_WAIT_MS: i32 = 1_000;
const FETCH_MIN_BYTES: i32 = 1;
const FETCH_MAX_BYTES: i32 = 1_000_000;
const PRODUCE_TIMEOUT: Duration = Duration::from_secs(2);
const TAIL_SETUP_TIMEOUT: Duration = Duration::from_secs(5);
const TOPIC_CREATE_TIMEOUT: Duration = Duration::from_secs(5);
const TOPIC_PARTITIONS: i32 = 1;
const TOPIC_REPLICATION_FACTOR: i16 = 1;

/// Current payload format version. Stored as the first byte of every record
/// so that future changes can be decoded without ambiguity.
const PAYLOAD_VERSION: u8 = 1;

/// Minimum payload size: 1 (version) + 8 (timestamp) + 1 (severity).
const MIN_PAYLOAD_LEN: usize = 10;

/// Kafka enforces a maximum topic name length of 249 characters.
const MAX_TOPIC_NAME_LEN: usize = 249;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_byte(self) -> u8 {
        match self {
            Severity::Info => b'i',
            Severity::Warning => b'w',
            Severity::Error => b'e',
        }
    }

    pub fn from_byte(value: u8) -> Option<Self> {
        match value {
            b'i' => Some(Severity::Info),
            b'w' => Some(Severity::Warning),
            b'e' => Some(Severity::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TailConfig {
    pub follow: bool,
    pub start_from: StartFrom,
}

impl Default for TailConfig {
    fn default() -> Self {
        Self {
            follow: true,
            start_from: StartFrom::End(0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StartFrom {
    /// Start reading from `n` records before the end of the log.
    /// `StartFrom::End(0)` means start at the current tail (only new records).
    End(u64),

    /// Start reading from the beginning, skipping `n` records.
    /// `StartFrom::Beginning(0)` means start at the very first record.
    Beginning(u64),
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("failed to build kafka client: {0}")]
    Kafka(#[from] rskafka::client::error::Error),
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("failed to write log to kafka: {0}")]
    Kafka(#[from] rskafka::client::error::Error),

    #[error("failed to encode current timestamp: {0}")]
    Time(#[from] std::time::SystemTimeError),

    #[error("timed out while publishing log to kafka")]
    ProduceTimeout,
}

#[derive(Debug, Error)]
pub enum TailError {
    #[error("failed kafka operation: {0}")]
    Kafka(#[from] rskafka::client::error::Error),

    #[error("timed out while preparing log tail")]
    SetupTimeout,

    #[error("log payload shorter than required {MIN_PAYLOAD_LEN} bytes")]
    InvalidPayload,

    #[error("invalid severity byte: {0}")]
    InvalidSeverity(u8),

    #[error("unsupported payload version: {0}")]
    UnsupportedVersion(u8),
}

#[derive(Debug, Error)]
pub enum NamespaceError {
    #[error("failed kafka operation: {0}")]
    Kafka(#[from] rskafka::client::error::Error),

    #[error("timed out while ensuring namespace topic")]
    EnsureTimeout,

    #[error("invalid namespace: {0}")]
    InvalidNamespace(String),
}

/// Builder for LDB Kafka clients.
///
/// # Kafka authentication
///
/// The current implementation connects to Kafka without TLS or SASL
/// authentication. In production, place Kafka behind a network boundary
/// (e.g. a private VPC) and use network-level access control. Adding
/// TLS/SASL support is tracked separately.
pub struct ClientBuilder {
    brokers: String,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self {
            brokers: DEFAULT_BROKERS.to_string(),
        }
    }

    pub fn brokers(mut self, brokers: impl Into<String>) -> Self {
        self.brokers = brokers.into();
        self
    }

    async fn connect(self) -> Result<Arc<rskafka::client::Client>, ConnectError> {
        let client = RskafkaClientBuilder::new(vec![self.brokers])
            .build()
            .await?;
        Ok(Arc::new(client))
    }

    pub async fn build_publisher(self) -> Result<Publisher, ConnectError> {
        Ok(Publisher {
            client: self.connect().await?,
        })
    }

    pub async fn build_consumer(self) -> Result<Consumer, ConnectError> {
        Ok(Consumer {
            client: self.connect().await?,
        })
    }
}

#[derive(Clone)]
pub struct Publisher {
    client: Arc<rskafka::client::Client>,
}

impl Publisher {
    pub async fn namespace(&self, namespace: String) -> Result<NamespacePublisher, NamespaceError> {
        let topic = topic_for_namespace(&namespace)?;
        ensure_topic(&self.client, &topic).await?;
        Ok(NamespacePublisher {
            client: self.client.clone(),
            topic,
            partition_client: Arc::new(Mutex::new(None)),
        })
    }
}

#[derive(Clone)]
pub struct NamespacePublisher {
    client: Arc<rskafka::client::Client>,
    topic: String,
    partition_client: Arc<Mutex<Option<Arc<rskafka::client::partition::PartitionClient>>>>,
}

impl NamespacePublisher {
    /// Log an informational message. This is a best-effort convenience method:
    /// errors are silently discarded. Use [`log`](Self::log) if you need error
    /// propagation.
    pub async fn info(&self, message: String) {
        let _ = self.log(Severity::Info, message).await;
    }

    /// Log a warning message. This is a best-effort convenience method:
    /// errors are silently discarded. Use [`log`](Self::log) if you need error
    /// propagation.
    pub async fn warn(&self, message: String) {
        let _ = self.log(Severity::Warning, message).await;
    }

    /// Log an error message. This is a best-effort convenience method:
    /// errors are silently discarded. Use [`log`](Self::log) if you need error
    /// propagation.
    pub async fn error(&self, message: String) {
        let _ = self.log(Severity::Error, message).await;
    }

    pub async fn log(&self, severity: Severity, message: String) -> Result<(), PublishError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let millis = now.as_millis() as u64;

        let payload = encode_payload(millis, severity, &message);

        let partition_client = self.get_or_create_partition_client().await?;

        tokio::time::timeout(
            PRODUCE_TIMEOUT,
            partition_client.produce(
                vec![Record {
                    key: None,
                    value: Some(payload),
                    headers: BTreeMap::new(),
                    timestamp: Utc::now(),
                }],
                Compression::NoCompression,
            ),
        )
        .await
        .map_err(|_| PublishError::ProduceTimeout)??;

        Ok(())
    }

    async fn get_or_create_partition_client(
        &self,
    ) -> Result<Arc<rskafka::client::partition::PartitionClient>, PublishError> {
        let mut guard = self.partition_client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(Arc::clone(client));
        }
        let client = tokio::time::timeout(
            PRODUCE_TIMEOUT,
            self.client
                .partition_client(&self.topic, 0, UnknownTopicHandling::Retry),
        )
        .await
        .map_err(|_| PublishError::ProduceTimeout)??;
        let client = Arc::new(client);
        *guard = Some(Arc::clone(&client));
        Ok(client)
    }
}

#[derive(Clone)]
pub struct Consumer {
    client: Arc<rskafka::client::Client>,
}

impl Consumer {
    pub async fn namespace(&self, namespace: String) -> Result<NamespaceConsumer, NamespaceError> {
        let topic = topic_for_namespace(&namespace)?;
        ensure_topic(&self.client, &topic).await?;
        Ok(NamespaceConsumer {
            client: self.client.clone(),
            topic,
        })
    }
}

#[derive(Clone)]
pub struct NamespaceConsumer {
    client: Arc<rskafka::client::Client>,
    topic: String,
}

impl NamespaceConsumer {
    pub async fn tail(
        &self,
        config: TailConfig,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<(u64, Severity, String), TailError>> + Send>>,
        TailError,
    > {
        let partition_client = tokio::time::timeout(
            TAIL_SETUP_TIMEOUT,
            self.client
                .partition_client(&self.topic, 0, UnknownTopicHandling::Retry),
        )
        .await
        .map_err(|_| TailError::SetupTimeout)??;

        let (start_offset, end_exclusive) = tokio::time::timeout(
            TAIL_SETUP_TIMEOUT,
            compute_offsets(&partition_client, config.start_from),
        )
        .await
        .map_err(|_| TailError::SetupTimeout)??;

        let follow = config.follow;
        let mut next_offset = start_offset;

        Ok(Box::pin(try_stream! {
            loop {
                if !follow && next_offset >= end_exclusive {
                    break;
                }

                let (records, _high_watermark) = partition_client
                    .fetch_records(
                        next_offset,
                        FETCH_MIN_BYTES..FETCH_MAX_BYTES,
                        FETCH_MAX_WAIT_MS,
                    )
                    .await?;

                if records.is_empty() {
                    if follow {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        continue;
                    }
                    break;
                }

                for record in records {
                    let (timestamp, severity, text) = decode_payload(&record)?;
                    next_offset = record.offset + 1;
                    yield (timestamp, severity, text);
                }
            }
        }))
    }
}

async fn ensure_topic(client: &rskafka::client::Client, topic: &str) -> Result<(), NamespaceError> {
    let controller = client.controller_client()?;
    let create = tokio::time::timeout(
        TOPIC_CREATE_TIMEOUT,
        controller.create_topic(
            topic.to_string(),
            TOPIC_PARTITIONS,
            TOPIC_REPLICATION_FACTOR,
            TOPIC_CREATE_TIMEOUT.as_millis() as i32,
        ),
    )
    .await
    .map_err(|_| NamespaceError::EnsureTimeout)?;

    match create {
        Ok(())
        | Err(KafkaError::ServerError {
            protocol_error: ProtocolError::TopicAlreadyExists,
            ..
        }) => Ok(()),
        Err(error) => Err(NamespaceError::Kafka(error)),
    }
}

async fn compute_offsets(
    partition_client: &rskafka::client::partition::PartitionClient,
    start_from: StartFrom,
) -> Result<(i64, i64), rskafka::client::error::Error> {
    let low = partition_client.get_offset(OffsetAt::Earliest).await?;
    let high = partition_client.get_offset(OffsetAt::Latest).await?;

    let start = match start_from {
        StartFrom::End(back) => {
            let back = i64::try_from(back).unwrap_or(i64::MAX);
            (high - back).max(low)
        }
        StartFrom::Beginning(skip) => {
            let skip = i64::try_from(skip).unwrap_or(i64::MAX);
            (low + skip).min(high)
        }
    };

    Ok((start, high))
}

/// Encode a log entry into the versioned binary payload format.
///
/// Layout (version 1):
///   byte 0:     version (0x01)
///   bytes 1-8:  timestamp in milliseconds since epoch (big-endian u64)
///   byte 9:     severity (ASCII: 'i', 'w', 'e')
///   bytes 10..: UTF-8 message
fn encode_payload(timestamp_millis: u64, severity: Severity, message: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(MIN_PAYLOAD_LEN + message.len());
    payload.push(PAYLOAD_VERSION);
    payload.extend_from_slice(&timestamp_millis.to_be_bytes());
    payload.push(severity.as_byte());
    payload.extend_from_slice(message.as_bytes());
    payload
}

fn decode_payload(record: &RecordAndOffset) -> Result<(u64, Severity, String), TailError> {
    let payload = record
        .record
        .value
        .as_deref()
        .ok_or(TailError::InvalidPayload)?;

    if payload.len() < MIN_PAYLOAD_LEN {
        return Err(TailError::InvalidPayload);
    }

    let version = payload[0];
    if version != PAYLOAD_VERSION {
        return Err(TailError::UnsupportedVersion(version));
    }

    let timestamp = u64::from_be_bytes(payload[1..9].try_into().unwrap());
    let severity = Severity::from_byte(payload[9]).ok_or(TailError::InvalidSeverity(payload[9]))?;
    let message = String::from_utf8_lossy(&payload[10..]).into_owned();

    Ok((timestamp, severity, message))
}

/// Validate a namespace string and derive its Kafka topic name.
///
/// Namespaces must be non-empty and the resulting topic name must not exceed
/// Kafka's 249-character limit.
fn topic_for_namespace(namespace: &str) -> Result<String, NamespaceError> {
    if namespace.is_empty() {
        return Err(NamespaceError::InvalidNamespace(
            "namespace must not be empty".to_string(),
        ));
    }

    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(namespace.as_bytes());
    let topic = format!("{TOPIC_PREFIX}{encoded}");

    if topic.len() > MAX_TOPIC_NAME_LEN {
        return Err(NamespaceError::InvalidNamespace(format!(
            "resulting topic name exceeds Kafka's {MAX_TOPIC_NAME_LEN}-character limit \
             (namespace is {} bytes)",
            namespace.len()
        )));
    }

    Ok(topic)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Severity --

    #[test]
    fn severity_roundtrip() {
        for sev in [Severity::Info, Severity::Warning, Severity::Error] {
            assert_eq!(Severity::from_byte(sev.as_byte()), Some(sev));
        }
    }

    #[test]
    fn severity_from_unknown_byte_is_none() {
        assert_eq!(Severity::from_byte(b'x'), None);
        assert_eq!(Severity::from_byte(0), None);
    }

    // -- Payload encoding/decoding --

    #[test]
    fn encode_decode_roundtrip() {
        let ts: u64 = 1_700_000_000_000;
        let sev = Severity::Warning;
        let msg = "hello world";

        let payload = encode_payload(ts, sev, msg);
        assert_eq!(payload.len(), MIN_PAYLOAD_LEN + msg.len());

        // Wrap in RecordAndOffset for decode
        let record = RecordAndOffset {
            record: Record {
                key: None,
                value: Some(payload),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            },
            offset: 0,
        };

        let (decoded_ts, decoded_sev, decoded_msg) = decode_payload(&record).unwrap();
        assert_eq!(decoded_ts, ts);
        assert_eq!(decoded_sev, sev);
        assert_eq!(decoded_msg, msg);
    }

    #[test]
    fn decode_rejects_short_payload() {
        let record = RecordAndOffset {
            record: Record {
                key: None,
                value: Some(vec![0; 5]),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            },
            offset: 0,
        };
        assert!(matches!(
            decode_payload(&record),
            Err(TailError::InvalidPayload)
        ));
    }

    #[test]
    fn decode_rejects_missing_payload() {
        let record = RecordAndOffset {
            record: Record {
                key: None,
                value: None,
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            },
            offset: 0,
        };
        assert!(matches!(
            decode_payload(&record),
            Err(TailError::InvalidPayload)
        ));
    }

    #[test]
    fn decode_rejects_unknown_version() {
        let mut payload = vec![0xFF]; // bad version
        payload.extend_from_slice(&0u64.to_be_bytes());
        payload.push(b'i');

        let record = RecordAndOffset {
            record: Record {
                key: None,
                value: Some(payload),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            },
            offset: 0,
        };
        assert!(matches!(
            decode_payload(&record),
            Err(TailError::UnsupportedVersion(0xFF))
        ));
    }

    #[test]
    fn decode_rejects_invalid_severity() {
        let mut payload = vec![PAYLOAD_VERSION];
        payload.extend_from_slice(&0u64.to_be_bytes());
        payload.push(b'z'); // bad severity

        let record = RecordAndOffset {
            record: Record {
                key: None,
                value: Some(payload),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            },
            offset: 0,
        };
        assert!(matches!(
            decode_payload(&record),
            Err(TailError::InvalidSeverity(b'z'))
        ));
    }

    #[test]
    fn decode_handles_invalid_utf8_lossily() {
        let mut payload = vec![PAYLOAD_VERSION];
        payload.extend_from_slice(&1000u64.to_be_bytes());
        payload.push(b'e');
        payload.extend_from_slice(&[0xFF, 0xFE]); // invalid UTF-8

        let record = RecordAndOffset {
            record: Record {
                key: None,
                value: Some(payload),
                headers: BTreeMap::new(),
                timestamp: Utc::now(),
            },
            offset: 0,
        };
        let (_, _, msg) = decode_payload(&record).unwrap();
        assert!(msg.contains('\u{FFFD}')); // replacement character
    }

    // -- Topic naming --

    #[test]
    fn topic_for_namespace_basic() {
        let topic = topic_for_namespace("org/repo/env/deploy").unwrap();
        assert!(topic.starts_with(TOPIC_PREFIX));
        // base64url encoding of "org/repo/env/deploy"
        let expected_encoded =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("org/repo/env/deploy");
        assert_eq!(topic, format!("{TOPIC_PREFIX}{expected_encoded}"));
    }

    #[test]
    fn topic_for_namespace_rejects_empty() {
        assert!(topic_for_namespace("").is_err());
    }

    #[test]
    fn topic_for_namespace_rejects_too_long() {
        // A namespace of 200 bytes base64-encodes to ~268 chars + 3-char prefix = 271
        let long_ns = "x".repeat(200);
        assert!(topic_for_namespace(&long_ns).is_err());
    }

    // -- StartFrom doc coverage (compile-time check via doc examples) --

    #[test]
    fn start_from_variants() {
        let _end = StartFrom::End(10);
        let _begin = StartFrom::Beginning(0);
    }
}
