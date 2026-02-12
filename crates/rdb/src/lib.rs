use std::sync::Arc;

use sclc::Record;
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::{ExecutionError, NewSessionError, PrepareError},
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
}

#[derive(Clone)]
pub struct ResourceClient {
    namespace: NamespaceClient,
    resource_type: String,
    id: String,
}

impl ResourceClient {
    pub async fn get(&self) -> Result<Resource, ReadError> {
        todo!()
    }

    pub async fn set_input(&self, inputs: Record) -> Result<Resource, ReadError> {
        todo!()
    }
}

pub struct Resource {}

pub enum ReadError {}
