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

prepared_statements! {
    KeyspaceStatements {
        create_keyspace = r#"
            CREATE KEYSPACE IF NOT EXISTS rdb
            WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}
        "#,
    }

    TableStatements {
        create_resources_table = r#"
            CREATE TABLE IF NOT EXISTS rdb.resources (
                namespace TEXT,
                resource_type TEXT,
                id TEXT,
                inputs_json TEXT,
                outputs_json TEXT,
                dependencies_json TEXT,
                markers_json TEXT,
                owner TEXT,
                PRIMARY KEY ((namespace), resource_type, id)
            )
        "#,
    }

    PreparedStatements {
        get_resource = r#"
            SELECT inputs_json, outputs_json, dependencies_json, markers_json, owner
            FROM rdb.resources
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
        list_resources = r#"
            SELECT resource_type, id, inputs_json, outputs_json, dependencies_json, markers_json, owner
            FROM rdb.resources
            WHERE namespace = ?
        "#,
        list_resources_by_owner = r#"
            SELECT resource_type, id, inputs_json, outputs_json, dependencies_json, markers_json, owner
            FROM rdb.resources
            WHERE namespace = ?
            AND owner = ?
            ALLOW FILTERING
        "#,
        set_resource_input = r#"
            UPDATE rdb.resources
            SET inputs_json = ?,
                owner = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
        set_resource_output = r#"
            UPDATE rdb.resources
            SET outputs_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
        set_resource_dependencies = r#"
            UPDATE rdb.resources
            SET dependencies_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
        set_resource_markers = r#"
            UPDATE rdb.resources
            SET markers_json = ?
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
        delete_resource = r#"
            DELETE FROM rdb.resources
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
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
);

fn map_resource_row(namespace: &str, row: ResourceRow) -> Result<Resource, ResourceError> {
    let (resource_type, id, inputs_json, outputs_json, dependencies_json, markers_json, owner) =
        row;
    Ok(Resource {
        namespace: namespace.to_owned(),
        resource_type,
        id,
        inputs: decode_record(inputs_json)?,
        outputs: decode_record(outputs_json)?,
        dependencies: decode_dependencies(dependencies_json)?,
        markers: decode_markers(markers_json)?,
        owner,
    })
}

#[derive(Default)]
pub struct ClientBuilder {
    inner: SessionBuilder,
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.inner = self.inner.known_node(hostname);
        self
    }

    pub async fn build(&self) -> Result<Client, ConnectError> {
        let session = Arc::new(self.inner.build().await?);

        let statements = KeyspaceStatements::new(&session).await?;

        session
            .execute_unpaged(&statements.create_keyspace, ())
            .await?;

        let statements = TableStatements::new(&session).await?;

        session
            .execute_unpaged(&statements.create_resources_table, ())
            .await?;

        let statements = PreparedStatements::new(&session).await?;

        Ok(Client {
            session,
            statements,
        })
    }
}

#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    statements: PreparedStatements,
}

impl Client {
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

    pub fn resource(&self, resource_type: String, id: String) -> ResourceClient {
        ResourceClient {
            namespace: self.clone(),
            resource_type,
            id,
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
        Ok(pager
            .rows_stream::<ResourceRow>()?
            .map(move |row| map_resource_row(&namespace, row?)))
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
        Ok(pager
            .rows_stream::<ResourceRow>()?
            .map(move |row| map_resource_row(&namespace, row?)))
    }
}

#[derive(Clone)]
pub struct ResourceClient {
    namespace: NamespaceClient,
    resource_type: String,
    id: String,
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
            self.id.as_str(),
        )
    }

    pub async fn get(&self) -> Result<Option<Resource>, ResourceError> {
        let pager = self
            .session()
            .execute_iter(self.statements().get_resource.clone(), self.key())
            .await?;

        let (inputs_json, outputs_json, dependencies_json, markers_json, owner) = match pager
            .rows_stream::<(
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
            resource_type: self.resource_type.clone(),
            id: self.id.clone(),
            inputs: decode_record(inputs_json)?,
            outputs: decode_record(outputs_json)?,
            dependencies: decode_dependencies(dependencies_json)?,
            markers: decode_markers(markers_json)?,
            owner,
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
        dependencies: &[sclc::ResourceId],
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
    pub resource_type: String,
    pub id: String,
    pub inputs: Option<Record>,
    pub outputs: Option<Record>,
    pub dependencies: Vec<sclc::ResourceId>,
    pub markers: BTreeSet<sclc::Marker>,
    pub owner: Option<String>,
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

    #[error("failed to encode json: {0}")]
    EncodeJson(#[from] serde_json::Error),
}

fn decode_record(value: Option<String>) -> Result<Option<Record>, ResourceError> {
    match value {
        None => Ok(None),
        Some(text) if text.is_empty() => Ok(None),
        Some(text) => Ok(Some(serde_json::from_str(&text)?)),
    }
}

fn decode_dependencies(value: Option<String>) -> Result<Vec<sclc::ResourceId>, ResourceError> {
    match value {
        None => Ok(Vec::new()),
        Some(text) if text.is_empty() => Ok(Vec::new()),
        Some(text) => Ok(serde_json::from_str(&text)?),
    }
}

fn decode_markers(value: Option<String>) -> Result<BTreeSet<sclc::Marker>, ResourceError> {
    match value {
        None => Ok(BTreeSet::new()),
        Some(text) if text.is_empty() => Ok(BTreeSet::new()),
        Some(text) => Ok(serde_json::from_str(&text)?),
    }
}

fn encode_record(value: &Record) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}

fn encode_dependencies(value: &[sclc::ResourceId]) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}

fn encode_markers(value: &BTreeSet<sclc::Marker>) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}
