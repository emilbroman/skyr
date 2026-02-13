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
    TableStatements {
        create_keyspace = r#"
            CREATE KEYSPACE IF NOT EXISTS rdb
            WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}
        "#,
        create_resources_table = r#"
            CREATE TABLE IF NOT EXISTS rdb.resources (
                namespace TEXT,
                resource_type TEXT,
                id TEXT,
                inputs_json TEXT,
                outputs_json TEXT,
                owner TEXT,
                PRIMARY KEY ((namespace), resource_type, id)
            )
        "#,
    }

    PreparedStatements {
        get_resource = r#"
            SELECT inputs_json, outputs_json, owner
            FROM rdb.resources
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
        list_resources = r#"
            SELECT resource_type, id, inputs_json, outputs_json, owner
            FROM rdb.resources
            WHERE namespace = ?
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
        delete_resource = r#"
            DELETE FROM rdb.resources
            WHERE namespace = ?
            AND resource_type = ?
            AND id = ?
        "#,
    }
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

        let statements = TableStatements::new(&session).await?;

        let (r0, r1) = futures::join!(
            session.execute_unpaged(&statements.create_keyspace, ()),
            session.execute_unpaged(&statements.create_resources_table, ()),
        );
        r0?;
        r1?;

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
            .rows_stream::<(String, String, Option<String>, Option<String>, Option<String>)>()?
            .map(move |row| {
                let (resource_type, id, inputs_json, outputs_json, owner) = row?;
                Ok::<_, ResourceError>(Resource {
                    namespace: namespace.clone(),
                    resource_type,
                    id,
                    inputs: decode_record(inputs_json)?,
                    outputs: decode_record(outputs_json)?,
                    owner,
                })
            }))
    }
}

#[derive(Clone)]
pub struct ResourceClient {
    namespace: NamespaceClient,
    resource_type: String,
    id: String,
}

impl ResourceClient {
    pub async fn get(&self) -> Result<Resource, ResourceError> {
        let pager = self
            .namespace
            .client
            .session
            .execute_iter(
                self.namespace.client.statements.get_resource.clone(),
                (
                    self.namespace.namespace.as_str(),
                    self.resource_type.as_str(),
                    self.id.as_str(),
                ),
            )
            .await?;

        let (inputs_json, outputs_json, owner) = match pager
            .rows_stream::<(Option<String>, Option<String>, Option<String>)>()?
            .try_next()
            .await?
        {
            Some(row) => row,
            None => return Err(ResourceError::NotFound),
        };

        Ok(Resource {
            namespace: self.namespace.namespace.clone(),
            resource_type: self.resource_type.clone(),
            id: self.id.clone(),
            inputs: decode_record(inputs_json)?,
            outputs: decode_record(outputs_json)?,
            owner,
        })
    }

    pub async fn set_input(&self, inputs: Record, owner: String) -> Result<Resource, ResourceError> {
        let inputs_json = encode_record(&inputs)?;

        self.namespace
            .client
            .session
            .execute_unpaged(
                &self.namespace.client.statements.set_resource_input,
                (
                    inputs_json,
                    owner.as_str(),
                    self.namespace.namespace.as_str(),
                    self.resource_type.as_str(),
                    self.id.as_str(),
                ),
            )
            .await?;

        self.get().await
    }

    pub async fn set_output(&self, outputs: Record) -> Result<Resource, ResourceError> {
        let outputs_json = encode_record(&outputs)?;

        self.namespace
            .client
            .session
            .execute_unpaged(
                &self.namespace.client.statements.set_resource_output,
                (
                    outputs_json,
                    self.namespace.namespace.as_str(),
                    self.resource_type.as_str(),
                    self.id.as_str(),
                ),
            )
            .await?;

        self.get().await
    }

    pub async fn delete(&self) -> Result<(), ResourceError> {
        self.namespace
            .client
            .session
            .execute_unpaged(
                &self.namespace.client.statements.delete_resource,
                (
                    self.namespace.namespace.as_str(),
                    self.resource_type.as_str(),
                    self.id.as_str(),
                ),
            )
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
    pub owner: Option<String>,
}

#[derive(Error, Debug)]
pub enum ResourceError {
    #[error("not found")]
    NotFound,

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

fn encode_record(value: &Record) -> Result<String, ResourceError> {
    Ok(serde_json::to_string(value)?)
}
