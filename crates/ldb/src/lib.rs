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
    End(u64),
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

    #[error("log payload shorter than required 9 bytes")]
    InvalidPayload,

    #[error("invalid severity byte: {0}")]
    InvalidSeverity(u8),

    #[error("invalid utf8 log message: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[derive(Debug, Error)]
pub enum NamespaceError {
    #[error("failed kafka operation: {0}")]
    Kafka(#[from] rskafka::client::error::Error),

    #[error("timed out while ensuring namespace topic")]
    EnsureTimeout,
}

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
        let topic = topic_for_namespace(&namespace);
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
    pub async fn info(&self, message: String) {
        self.log(Severity::Info, message).await.unwrap_or_default();
    }

    pub async fn warn(&self, message: String) {
        self.log(Severity::Warning, message)
            .await
            .unwrap_or_default();
    }

    pub async fn error(&self, message: String) {
        self.log(Severity::Error, message).await.unwrap_or_default();
    }

    pub async fn log(&self, severity: Severity, message: String) -> Result<(), PublishError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let millis = now.as_millis() as u64;

        let mut payload = Vec::with_capacity(9 + message.len());
        payload.extend_from_slice(&millis.to_be_bytes());
        payload.push(severity.as_byte());
        payload.extend_from_slice(message.as_bytes());

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
        let topic = topic_for_namespace(&namespace);
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

fn decode_payload(record: &RecordAndOffset) -> Result<(u64, Severity, String), TailError> {
    let payload = record
        .record
        .value
        .as_deref()
        .ok_or(TailError::InvalidPayload)?;

    if payload.len() < 9 {
        return Err(TailError::InvalidPayload);
    }

    let timestamp = u64::from_be_bytes(payload[..8].try_into().unwrap());

    let severity = Severity::from_byte(payload[8]).ok_or(TailError::InvalidSeverity(payload[8]))?;
    let message = String::from_utf8(payload[9..].to_vec())?;

    Ok((timestamp, severity, message))
}

fn topic_for_namespace(namespace: &str) -> String {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(namespace.as_bytes());
    format!("{TOPIC_PREFIX}{encoded}")
}
