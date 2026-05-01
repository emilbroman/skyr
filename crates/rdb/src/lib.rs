use std::collections::BTreeSet;
use std::sync::Arc;

use futures::{Stream, StreamExt, TryStreamExt};
use sclc::Record;
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::{
        ExecutionError, NewSessionError, NextRowError, PagerExecutionError, PrepareError,
        TypeCheckError,
    },
    statement::prepared::PreparedStatement,
};
use thiserror::Error;

/// Maximum size in bytes for JSON payloads deserialized from the database.
///
/// This prevents malicious or corrupted database content from causing
/// unbounded memory allocation during deserialization.
const MAX_JSON_SIZE: usize = 16 * 1024 * 1024; // 16 MiB

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create session: {0}")]
    Scylla(#[from] NewSessionError),

    #[error("failed to prepare statement: {0}")]
    Prepare(#[from] PrepareError),

    #[error("failed to create tables: {0}")]
    CreateTables(#[from] ExecutionError),
}

macro_rules! prepared_statements {
    ($($struct_name:ident { $($name:ident = $statement:expr,)* })+) => {
        $(
            #[derive(Clone)]
            struct $struct_name {
                $($name: PreparedStatement,)*
            }

            impl $struct_name {
                async fn new(session: &Session) -> Result<Self, PrepareError> {
                    let ($($name,)*) = futures::join!(
                        $(session.prepare($statement)),*
                    );

                    Ok(Self {
                        $($name: $name?,)*
                    })
                }
            }
        )+
    }
}

// Schema version: 1
//
// Migration strategy: this crate uses CREATE ... IF NOT EXISTS for all DDL.
// When the schema needs to change, add new CREATE statements (for additive
// changes like new columns or indexes) or implement an explicit migration
// step that checks the current schema version and applies ALTER statements.
// Bump the version comment above when the schema changes.

prepared_statements! {
    TableStatements {
        create_resources_table = r#"
            CREATE TABLE IF NOT EXISTS rdb.resources (
                namespace TEXT,
                resource_type TEXT,
                name TEXT,
                inputs_json TEXT,
                outputs_json TEXT,
                dependencies_json TEXT,
                markers_json TEXT,
                owner TEXT,
                source_trace_json TEXT,
                PRIMARY KEY ((namespace), resource_type, name)
            )
        "#,
        create_owner_index = r#"
            CREATE INDEX IF NOT EXISTS resources_owner_idx
            ON rdb.resources (owner)
        "#,
    }

    PreparedStatements {
        get_resource = r#"
            SELECT inputs_json, outputs_json, dependencies_json, markers_json, owner, source_trace_json
            FROM rdb.resources
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
        list_resources = r#"
            SELECT resource_type, name, inputs_json, outputs_json, dependencies_json, markers_json, owner, source_trace_json
            FROM rdb.resources
            WHERE namespace = ?
        "#,
        list_resources_by_owner = r#"
            SELECT resource_type, name, inputs_json, outputs_json, dependencies_json, markers_json, owner, source_trace_json
            FROM rdb.resources
            WHERE namespace = ?
            AND owner = ?
        "#,
        set_resource_input = r#"
            UPDATE rdb.resources
            SET inputs_json = ?,
                owner = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
        set_resource_output = r#"
            UPDATE rdb.resources
            SET outputs_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
        set_resource_dependencies = r#"
            UPDATE rdb.resources
            SET dependencies_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
        set_resource_markers = r#"
            UPDATE rdb.resources
            SET markers_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
        set_resource_source_trace = r#"
            UPDATE rdb.resources
            SET source_trace_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
        delete_resource = r#"
            DELETE FROM rdb.resources
            WHERE namespace = ?
            AND resource_type = ?
            AND name = ?
        "#,
    }
}

type ResourceRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn map_resource_row(
    namespace: &str,
    region: &ids::RegionId,
    row: ResourceRow,
) -> Result<Resource, ResourceError> {
    let (
        resource_type,
        name,
        inputs_json,
        outputs_json,
        dependencies_json,
        markers_json,
        owner,
        source_trace_json,
    ) = row;
    Ok(Resource {
        namespace: namespace.to_owned(),
        region: region.clone(),
        resource_type,
        name,
        inputs: decode_record(inputs_json)?,
        outputs: decode_record(outputs_json)?,
        dependencies: decode_dependencies(dependencies_json)?,
        markers: decode_markers(markers_json)?,
        owner,
        source_trace: decode_source_trace(source_trace_json)?,
    })
}

#[derive(Default)]
pub struct ClientBuilder {
    inner: SessionBuilder,
    replication_factor: Option<u32>,
    region: Option<ids::RegionId>,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.inner = self.inner.known_node(hostname);
        self
    }

    /// Sets the replication factor for the `rdb` keyspace.
    ///
    /// Defaults to 1 if not specified. Production deployments should use a
    /// higher value (e.g. 3) for redundancy.
    pub fn replication_factor(mut self, factor: u32) -> Self {
        self.replication_factor = Some(factor);
        self
    }

    /// Sets the region this RDB client serves. RDB is sharded by Skyr
    /// region (every region has its own RDB), so every row this client
    /// reads or writes is, by definition, in this region. The region
    /// value isn't stored on the row itself — it's stamped onto returned
    /// [`Resource`] values at read time so callers can reconstruct the
    /// region-prefixed [`ids::ResourceId`].
    pub fn region(mut self, region: ids::RegionId) -> Self {
        self.region = Some(region);
        self
    }

    pub async fn build(&self) -> Result<Client, ConnectError> {
        let session = Arc::new(self.inner.build().await?);

        let replication_factor = self.replication_factor.unwrap_or(1);
        let create_keyspace = format!(
            "CREATE KEYSPACE IF NOT EXISTS rdb \
             WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': {replication_factor}}}"
        );
        session.query_unpaged(create_keyspace, ()).await?;

        let statements = TableStatements::new(&session).await?;

        session
            .execute_unpaged(&statements.create_resources_table, ())
            .await?;

        session
            .execute_unpaged(&statements.create_owner_index, ())
            .await?;

        let statements = PreparedStatements::new(&session).await?;

        let region = self
            .region
            .clone()
            .expect("ClientBuilder::region must be set before build");

        Ok(Client {
            session,
            statements,
            region,
        })
    }
}

#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    statements: PreparedStatements,
    region: ids::RegionId,
}

impl Client {
    /// The region of this RDB client. All resources read or written
    /// through this client are in this region.
    pub fn region(&self) -> &ids::RegionId {
        &self.region
    }

    pub fn namespace(&self, namespace: String) -> NamespaceClient {
        NamespaceClient {
            client: self.clone(),
            namespace,
        }
    }
}

#[derive(Clone)]
pub struct NamespaceClient {
    client: Client,
    namespace: String,
}

impl NamespaceClient {
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn resource(&self, resource_type: String, name: String) -> ResourceClient {
        ResourceClient {
            namespace: self.clone(),
            resource_type,
            name,
        }
    }

    pub async fn list_resources(
        &self,
    ) -> Result<impl Stream<Item = Result<Resource, ResourceError>>, ResourceError> {
        let pager = self
            .client
            .session
            .execute_iter(
                self.client.statements.list_resources.clone(),
                (self.namespace.as_str(),),
            )
            .await?;

        let namespace = self.namespace.clone();
        let region = self.client.region.clone();
        Ok(pager
            .rows_stream::<ResourceRow>()?
            .map(move |row| map_resource_row(&namespace, &region, row?)))
    }

    pub async fn list_resources_by_owner(
        &self,
        owner: &str,
    ) -> Result<impl Stream<Item = Result<Resource, ResourceError>>, ResourceError> {
        let pager = self
            .client
            .session
            .execute_iter(
                self.client.statements.list_resources_by_owner.clone(),
                (self.namespace.as_str(), owner),
            )
            .await?;

        let namespace = self.namespace.clone();
        let region = self.client.region.clone();
        Ok(pager
            .rows_stream::<ResourceRow>()?
            .map(move |row| map_resource_row(&namespace, &region, row?)))
    }
}

#[derive(Clone)]
pub struct ResourceClient {
    namespace: NamespaceClient,
    resource_type: String,
    name: String,
}

impl ResourceClient {
    fn session(&self) -> &Session {
        &self.namespace.client.session
    }

    fn statements(&self) -> &PreparedStatements {
        &self.namespace.client.statements
    }

    fn key(&self) -> (&str, &str, &str) {
        (
            self.namespace.namespace.as_str(),
            self.resource_type.as_str(),
            self.name.as_str(),
        )
    }

    pub async fn get(&self) -> Result<Option<Resource>, ResourceError> {
        let pager = self
            .session()
            .execute_iter(self.statements().get_resource.clone(), self.key())
            .await?;

        let (inputs_json, outputs_json, dependencies_json, markers_json, owner, source_trace_json) =
            match pager
                .rows_stream::<(
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                    Option<String>,
                )>()?
                .try_next()
                .await?
            {
                Some(row) => row,
                None => return Ok(None),
            };

        Ok(Some(Resource {
            namespace: self.namespace.namespace.clone(),
            region: self.namespace.client.region.clone(),
            resource_type: self.resource_type.clone(),
            name: self.name.clone(),
            inputs: decode_record(inputs_json)?,
            outputs: decode_record(outputs_json)?,
            dependencies: decode_dependencies(dependencies_json)?,
            markers: decode_markers(markers_json)?,
            owner,
            source_trace: decode_source_trace(source_trace_json)?,
        }))
    }

    pub async fn set_input(
        &self,
        inputs: Record,
        owner: String,
    ) -> Result<Resource, ResourceError> {
        let inputs_json = encode_record(&inputs)?;
        let (namespace, resource_type, id) = self.key();

        self.session()
            .execute_unpaged(
                &self.statements().set_resource_input,
                (inputs_json, owner.as_str(), namespace, resource_type, id),
            )
            .await?;

        self.get().await?.ok_or(ResourceError::MissingAfterWrite)
    }

    pub async fn set_output(&self, outputs: Record) -> Result<Resource, ResourceError> {
        let outputs_json = encode_record(&outputs)?;
        let (namespace, resource_type, id) = self.key();

        self.session()
            .execute_unpaged(
                &self.statements().set_resource_output,
                (outputs_json, namespace, resource_type, id),
            )
            .await?;

        self.get().await?.ok_or(ResourceError::MissingAfterWrite)
    }

    pub async fn set_dependencies(
        &self,
        dependencies: &[ids::ResourceId],
    ) -> Result<Resource, ResourceError> {
        let dependencies_json = encode_dependencies(dependencies)?;
        let (namespace, resource_type, id) = self.key();

        self.session()
            .execute_unpaged(
                &self.statements().set_resource_dependencies,
                (dependencies_json, namespace, resource_type, id),
            )
            .await?;

        self.get().await?.ok_or(ResourceError::MissingAfterWrite)
    }

    pub async fn set_markers(
        &self,
        markers: &BTreeSet<sclc::Marker>,
    ) -> Result<Resource, ResourceError> {
        let markers_json = encode_markers(markers)?;
        let (namespace, resource_type, id) = self.key();

        self.session()
            .execute_unpaged(
                &self.statements().set_resource_markers,
                (markers_json, namespace, resource_type, id),
            )
            .await?;

        self.get().await?.ok_or(ResourceError::MissingAfterWrite)
    }

    pub async fn set_source_trace(
        &self,
        source_trace: &ids::SourceTrace,
    ) -> Result<Resource, ResourceError> {
        let source_trace_json = encode_source_trace(source_trace)?;
        let (namespace, resource_type, id) = self.key();

        self.session()
            .execute_unpaged(
                &self.statements().set_resource_source_trace,
                (source_trace_json, namespace, resource_type, id),
            )
            .await?;

        self.get().await?.ok_or(ResourceError::MissingAfterWrite)
    }

    pub async fn delete(&self) -> Result<(), ResourceError> {
        self.session()
            .execute_unpaged(&self.statements().delete_resource, self.key())
            .await?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct Resource {
    pub namespace: String,
    /// The Skyr region this resource lives in. Stamped at read time from
    /// the [`Client`]'s region — RDB is sharded by region (every
    /// region has its own RDB), so the value is always the region of
    /// the connection that produced this row.
    pub region: ids::RegionId,
    pub resource_type: String,
    pub name: String,
    pub inputs: Option<Record>,
    pub outputs: Option<Record>,
    pub dependencies: Vec<ids::ResourceId>,
    pub markers: BTreeSet<sclc::Marker>,
    pub owner: Option<String>,
    pub source_trace: ids::SourceTrace,
}

#[derive(Error, Debug)]
pub enum ResourceError {
    #[error("resource missing after successful write")]
    MissingAfterWrite,

    #[error("failed to execute: {0}")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("failed to execute statement: {0}")]
    ScyllaExecute(#[from] ExecutionError),

    #[error("failed to parse row: {0}")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    ScyllaNextRow(#[from] NextRowError),

    #[error("failed to encode/decode json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("JSON payload too large ({size} bytes, limit is {MAX_JSON_SIZE})")]
    JsonTooLarge { size: usize },
}

/// Checks that a JSON string does not exceed [`MAX_JSON_SIZE`] before
/// deserializing it. Returns [`ResourceError::JsonTooLarge`] if the payload
/// is too large.
fn check_json_size(text: &str) -> Result<(), ResourceError> {
    if text.len() > MAX_JSON_SIZE {
        return Err(ResourceError::JsonTooLarge { size: text.len() });
    }
    Ok(())
}

/// Decodes a JSON-encoded [`Record`] from an optional database column.
///
/// Both SQL `NULL` and empty strings are treated as `None`, since ScyllaDB
/// may store either representation for absent values.
fn decode_record(value: Option<String>) -> Result<Option<Record>, ResourceError> {
    match value {
        None => Ok(None),
        Some(text) if text.is_empty() => Ok(None),
        Some(text) => {
            check_json_size(&text)?;
            Ok(Some(serde_json::from_str(&text)?))
        }
    }
}

/// Decodes a JSON-encoded list of [`ids::ResourceId`] from an optional
/// database column.
///
/// Both SQL `NULL` and empty strings are treated as an empty `Vec`, since
/// ScyllaDB may store either representation for absent values.
fn decode_dependencies(value: Option<String>) -> Result<Vec<ids::ResourceId>, ResourceError> {
    match value {
        None => Ok(Vec::new()),
        Some(text) if text.is_empty() => Ok(Vec::new()),
        Some(text) => {
            check_json_size(&text)?;
            Ok(serde_json::from_str(&text)?)
        }
    }
}

/// Decodes a JSON-encoded set of [`sclc::Marker`] from an optional database
/// column.
///
/// Both SQL `NULL` and empty strings are treated as an empty set, since
/// ScyllaDB may store either representation for absent values.
fn decode_markers(value: Option<String>) -> Result<BTreeSet<sclc::Marker>, ResourceError> {
    match value {
        None => Ok(BTreeSet::new()),
        Some(text) if text.is_empty() => Ok(BTreeSet::new()),
        Some(text) => {
            check_json_size(&text)?;
            Ok(serde_json::from_str(&text)?)
        }
    }
}

/// Serializes a [`Record`] to a JSON string for database storage.
fn encode_record(value: &Record) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}

/// Serializes a slice of [`ids::ResourceId`] to a JSON string for database storage.
fn encode_dependencies(value: &[ids::ResourceId]) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}

/// Serializes a set of [`sclc::Marker`] to a JSON string for database storage.
fn encode_markers(value: &BTreeSet<sclc::Marker>) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}

/// Decodes a JSON-encoded [`ids::SourceTrace`] from an optional database
/// column.
///
/// Both SQL `NULL` and empty strings are treated as an empty trace, since
/// ScyllaDB may store either representation for absent values.
fn decode_source_trace(value: Option<String>) -> Result<ids::SourceTrace, ResourceError> {
    match value {
        None => Ok(Vec::new()),
        Some(text) if text.is_empty() => Ok(Vec::new()),
        Some(text) => {
            check_json_size(&text)?;
            Ok(serde_json::from_str(&text)?)
        }
    }
}

/// Serializes a [`ids::SourceTrace`] to a JSON string for database storage.
fn encode_source_trace(value: &ids::SourceTrace) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── decode_record ──────────────────────────────────────────────

    #[test]
    fn decode_record_none_returns_none() {
        assert!(decode_record(None).unwrap().is_none());
    }

    #[test]
    fn decode_record_empty_string_returns_none() {
        assert!(decode_record(Some(String::new())).unwrap().is_none());
    }

    #[test]
    fn decode_record_valid_json_round_trips() {
        let mut record = Record::default();
        record.insert("key".into(), sclc::Value::Str("value".into()));
        let json = serde_json::to_string(&record).unwrap();
        let decoded = decode_record(Some(json)).unwrap().unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn decode_record_invalid_json_returns_error() {
        assert!(decode_record(Some("not json".into())).is_err());
    }

    // ── decode_dependencies ────────────────────────────────────────

    #[test]
    fn decode_dependencies_none_returns_empty() {
        assert!(decode_dependencies(None).unwrap().is_empty());
    }

    #[test]
    fn decode_dependencies_empty_string_returns_empty() {
        assert!(decode_dependencies(Some(String::new())).unwrap().is_empty());
    }

    #[test]
    fn decode_dependencies_valid_json_round_trips() {
        let region: ids::RegionId = "stockholm".parse().unwrap();
        let deps = vec![ids::ResourceId::new(region, "Std/Random.Int", "my-int")];
        let json = serde_json::to_string(&deps).unwrap();
        let decoded = decode_dependencies(Some(json)).unwrap();
        assert_eq!(decoded, deps);
    }

    // ── decode_markers ─────────────────────────────────────────────

    #[test]
    fn decode_markers_none_returns_empty() {
        assert!(decode_markers(None).unwrap().is_empty());
    }

    #[test]
    fn decode_markers_empty_string_returns_empty() {
        assert!(decode_markers(Some(String::new())).unwrap().is_empty());
    }

    #[test]
    fn decode_markers_valid_json_round_trips() {
        let mut markers = BTreeSet::new();
        markers.insert(sclc::Marker::Volatile);
        let json = serde_json::to_string(&markers).unwrap();
        let decoded = decode_markers(Some(json)).unwrap();
        assert_eq!(decoded, markers);
    }

    // ── decode_source_trace ────────────────────────────────────────

    #[test]
    fn decode_source_trace_none_returns_empty() {
        assert!(decode_source_trace(None).unwrap().is_empty());
    }

    #[test]
    fn decode_source_trace_empty_string_returns_empty() {
        assert!(decode_source_trace(Some(String::new())).unwrap().is_empty());
    }

    // ── encode round-trips ─────────────────────────────────────────

    #[test]
    fn encode_decode_record_round_trip() {
        let mut record = Record::default();
        record.insert("a".into(), sclc::Value::Int(42));
        let json = encode_record(&record).unwrap();
        let decoded = decode_record(Some(json)).unwrap().unwrap();
        assert_eq!(decoded, record);
    }

    #[test]
    fn encode_decode_dependencies_round_trip() {
        let region: ids::RegionId = "stockholm".parse().unwrap();
        let deps = vec![
            ids::ResourceId::new(region.clone(), "Std/Random.Int", "a"),
            ids::ResourceId::new(region, "Std/Time.Schedule", "b"),
        ];
        let json = encode_dependencies(&deps).unwrap();
        let decoded = decode_dependencies(Some(json)).unwrap();
        assert_eq!(decoded, deps);
    }

    #[test]
    fn encode_decode_markers_round_trip() {
        let mut markers = BTreeSet::new();
        markers.insert(sclc::Marker::Volatile);
        markers.insert(sclc::Marker::Sticky);
        let json = encode_markers(&markers).unwrap();
        let decoded = decode_markers(Some(json)).unwrap();
        assert_eq!(decoded, markers);
    }

    #[test]
    fn encode_decode_source_trace_round_trip() {
        let trace: ids::SourceTrace = vec![];
        let json = encode_source_trace(&trace).unwrap();
        let decoded = decode_source_trace(Some(json)).unwrap();
        assert_eq!(decoded, trace);
    }

    // ── size limit ─────────────────────────────────────────────────

    #[test]
    fn decode_record_rejects_oversized_payload() {
        let huge = "x".repeat(MAX_JSON_SIZE + 1);
        match decode_record(Some(huge)) {
            Err(ResourceError::JsonTooLarge { .. }) => {}
            other => panic!("expected JsonTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn decode_dependencies_rejects_oversized_payload() {
        let huge = "x".repeat(MAX_JSON_SIZE + 1);
        match decode_dependencies(Some(huge)) {
            Err(ResourceError::JsonTooLarge { .. }) => {}
            other => panic!("expected JsonTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn decode_markers_rejects_oversized_payload() {
        let huge = "x".repeat(MAX_JSON_SIZE + 1);
        match decode_markers(Some(huge)) {
            Err(ResourceError::JsonTooLarge { .. }) => {}
            other => panic!("expected JsonTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn decode_source_trace_rejects_oversized_payload() {
        let huge = "x".repeat(MAX_JSON_SIZE + 1);
        match decode_source_trace(Some(huge)) {
            Err(ResourceError::JsonTooLarge { .. }) => {}
            other => panic!("expected JsonTooLarge, got {other:?}"),
        }
    }
}
