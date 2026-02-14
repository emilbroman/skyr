use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    DeploymentId,
    deployment::{Deployment, InvalidDeploymentState},
};
use crate::{
    DeploymentState,
    repository_name::{InvalidRepositoryName, RepositoryName},
};
use chrono::{DateTime, Utc};
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt, TryStreamExt, stream};
use gix_hash::ObjectId;
use gix_object::{Blob, Commit, Object, Tree, WriteTo};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::{
        ExecutionError, IntoRowsResultError, NewSessionError, NextRowError, PagerExecutionError,
        PrepareError, SingleRowError, TypeCheckError,
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
            CREATE KEYSPACE IF NOT EXISTS cdb
            WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1}
        "#,
    }

    TableStatements {
        create_deployments_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.deployments (
                repository TEXT,
                ref_name TEXT,
                commit_hash BLOB,
                created_at TIMESTAMP,
                state TEXT,
                PRIMARY KEY ((repository), ref_name, commit_hash)
            )
        "#,

        create_active_deployments_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.active_deployments (
                repository TEXT,
                ref_name TEXT,
                commit_hash BLOB,
                deployment_id TEXT,
                PRIMARY KEY ((repository), ref_name, commit_hash)
            )
        "#,

        create_deployments_by_id_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.deployments_by_id (
                deployment_id TEXT,
                repository TEXT,
                ref_name TEXT,
                commit_hash BLOB,
                created_at TIMESTAMP,
                state TEXT,
                PRIMARY KEY ((deployment_id))
            )
        "#,

        create_objects_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.objects (
                repository TEXT,
                hash BLOB,
                contents BLOB,
                PRIMARY KEY ((repository), hash)
            )
        "#,

        create_supercessions_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.supercessions (
                repository TEXT,
                ref_name TEXT,
                superceding_commit_hash BLOB,
                superceded_commit_hash BLOB,
                PRIMARY KEY ((repository), ref_name, superceding_commit_hash)
            )
        "#,
    }

    PreparedStatements {
        read_object = r#"
            SELECT contents FROM cdb.objects
            WHERE repository = ?
            AND hash = ?
        "#,

        write_object = r#"
            INSERT INTO cdb.objects (repository, hash, contents)
            VALUES (?, ?, ?)
        "#,

        find_deployment = r#"
            SELECT created_at, state FROM cdb.deployments
            WHERE repository = ?
            AND ref_name = ?
            AND commit_hash = ?
        "#,

        find_deployments_by_ids = r#"
            SELECT repository, ref_name, commit_hash, created_at, state
            FROM cdb.deployments_by_id
            WHERE deployment_id IN ?
        "#,

        list_deployments_by_repo = r#"
            SELECT ref_name, commit_hash, created_at, state
            FROM cdb.deployments
            WHERE repository = ?
        "#,

        list_active_deployments = r#"
            SELECT deployment_id
            FROM cdb.active_deployments
        "#,

        list_active_deployments_by_repo = r#"
            SELECT deployment_id
            FROM cdb.active_deployments
            WHERE repository = ?
        "#,

        set_deployment = r#"
            INSERT INTO cdb.deployments (repository, ref_name, commit_hash, created_at, state)
            VALUES (?, ?, ?, ?, ?)
        "#,

        set_deployment_by_id = r#"
            INSERT INTO cdb.deployments_by_id (
                deployment_id,
                repository,
                ref_name,
                commit_hash,
                created_at,
                state
            )
            VALUES (?, ?, ?, ?, ?, ?)
        "#,

        set_active_deployment = r#"
            INSERT INTO cdb.active_deployments (repository, ref_name, commit_hash, deployment_id)
            VALUES (?, ?, ?, ?)
        "#,

        unset_active_deployment = r#"
            DELETE FROM cdb.active_deployments
            WHERE repository = ?
            AND ref_name = ?
            AND commit_hash = ?
        "#,

        create_supercession = r#"
            UPDATE cdb.supercessions
            SET superceded_commit_hash = ?
            WHERE repository = ?
            AND ref_name = ?
            AND superceding_commit_hash = ?
        "#,

        get_superceded_commit = r#"
            SELECT superceded_commit_hash
            FROM cdb.supercessions
            WHERE repository = ?
            AND ref_name = ?
            AND superceding_commit_hash = ?
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

        let statements = KeyspaceStatements::new(&session).await?;

        session
            .execute_unpaged(&statements.create_keyspace, ())
            .await?;

        let statements = TableStatements::new(&session).await?;

        let (r0, r1, r2, r3, r4) = futures::join!(
            session.execute_unpaged(&statements.create_deployments_table, ()),
            session.execute_unpaged(&statements.create_active_deployments_table, ()),
            session.execute_unpaged(&statements.create_deployments_by_id_table, ()),
            session.execute_unpaged(&statements.create_objects_table, ()),
            session.execute_unpaged(&statements.create_supercessions_table, ()),
        );
        r0?;
        r1?;
        r2?;
        r3?;
        r4?;

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
    pub fn repo(&self, name: RepositoryName) -> RepositoryClient {
        RepositoryClient {
            client: self.clone(),
            name,
        }
    }
}

#[derive(Clone)]
pub struct RepositoryClient {
    client: Client,
    name: RepositoryName,
}

impl RepositoryClient {
    pub fn deployment(&self, id: DeploymentId) -> DeploymentClient {
        DeploymentClient {
            repo: self.clone(),
            id,
        }
    }
}

#[derive(Error, Debug)]
pub enum LoadObjectError {
    #[error("not found")]
    NotFound,

    #[error("failed to execute: {0}")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("failed to parse row: {0}")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    ScyllaNextRow(#[from] NextRowError),
}

impl RepositoryClient {
    async fn read_object(&self, hash: ObjectId) -> Result<Vec<u8>, LoadObjectError> {
        let pager = self
            .client
            .session
            .execute_iter(
                self.client.statements.read_object.clone(),
                (self.name.to_string(), hash.as_bytes()),
            )
            .await?;

        match pager.rows_stream::<(Vec<u8>,)>()?.try_next().await? {
            None => Err(LoadObjectError::NotFound),
            Some((contents,)) => Ok(contents),
        }
    }
}

#[derive(Error, Debug)]
pub enum ReadObjectError {
    #[error("read failed: {0}")]
    Read(#[from] LoadObjectError),

    #[error("decode failed: {0}")]
    Decode(#[from] gix_object::decode::Error),
}

impl RepositoryClient {
    pub async fn write_object(&self, id: ObjectId, object: Object) -> Result<(), WriteObjectError> {
        let mut data = vec![];
        object.write_to(&mut data)?;

        self.client
            .session
            .execute_unpaged(
                &self.client.statements.write_object,
                (self.name.to_string(), id.as_slice(), data),
            )
            .await?;

        Ok(())
    }

    pub async fn read_raw_object(&self, hash: ObjectId) -> Result<Vec<u8>, LoadObjectError> {
        self.read_object(hash).await
    }
}

#[derive(Error, Debug)]
pub enum WriteObjectError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl RepositoryClient {
    pub async fn write_commit(&self, id: ObjectId, object: Commit) -> Result<(), WriteObjectError> {
        self.write_object(id, Object::Commit(object)).await
    }

    pub async fn read_commit(&self, hash: ObjectId) -> Result<Commit, ReadObjectError> {
        let data = self.read_object(hash).await?;
        let commit = gix_object::CommitRef::from_bytes(&data)?;
        Ok(commit.into_owned()?)
    }

    pub async fn write_tree(&self, id: ObjectId, object: Tree) -> Result<(), WriteObjectError> {
        self.write_object(id, Object::Tree(object)).await
    }

    pub async fn read_tree(&self, hash: ObjectId) -> Result<Tree, ReadObjectError> {
        let data = self.read_object(hash).await?;
        let tree = gix_object::TreeRef::from_bytes(&data)?;
        Ok(tree.into_owned())
    }

    pub async fn write_blob(&self, id: ObjectId, object: Blob) -> Result<(), WriteObjectError> {
        self.write_object(id, Object::Blob(object)).await
    }

    pub async fn read_blob(&self, hash: ObjectId) -> Result<Blob, ReadObjectError> {
        let data = self.read_object(hash).await?;
        let blob = gix_object::BlobRef::from_bytes(&data).unwrap(); // infallible
        Ok(blob.into_owned())
    }
}

#[derive(Clone)]
pub struct DeploymentClient {
    repo: RepositoryClient,
    id: DeploymentId,
}

impl DeploymentClient {
    pub fn repository_name(&self) -> &RepositoryName {
        &self.repo.name
    }

    pub fn fqid(&self) -> String {
        format!("{}/{}", self.repo.name, self.id)
    }

    pub async fn get(&self) -> Result<Deployment, DeploymentQueryError> {
        self.repo.client.deployment(&self.repo.name, &self.id).await
    }

    pub async fn set(&self, state: DeploymentState) -> Result<(), SetDeploymentError> {
        let prev_state = self.get().await.ok();

        if let (
            Some(Deployment {
                state: DeploymentState::Down,
                ..
            }),
            DeploymentState::Undesired,
        ) = (&prev_state, state)
        {
            // Don't set DOWN state to UNDESIRED
            return Ok(());
        }

        let deployment = Deployment {
            repository: self.repo.name.clone(),
            id: self.id.clone(),
            created_at: prev_state
                .as_ref()
                .map(|s| s.created_at.clone())
                .unwrap_or_else(|| Utc::now()),
            state,
        };

        self.repo.client.set_deployment(deployment).await?;

        Ok(())
    }

    pub async fn read_dir(&self, path: Option<impl AsRef<Path>>) -> Result<Tree, FileError> {
        let commit = self.repo.read_commit(self.id.commit_hash).await?;
        let mut tree = self.repo.read_tree(commit.tree).await?;

        let mut result_buf = PathBuf::new();

        let mut ancestors = vec![];
        if let Some(path) = path {
            for segment in path.as_ref() {
                if segment == "." {
                    continue;
                }

                if segment == ".." {
                    tree = ancestors.pop().unwrap_or(tree);
                }

                result_buf.push(segment);

                match tree
                    .entries
                    .iter()
                    .find(|e| e.filename.as_slice() == segment.as_encoded_bytes())
                {
                    None => return Err(FileError::NotFound(result_buf)),
                    Some(entry) => {
                        if !entry.mode.is_tree() {
                            return Err(FileError::NotADirectory(result_buf));
                        }

                        ancestors.push(tree.clone());
                        tree = self.repo.read_tree(entry.oid).await?;
                    }
                }
            }
        }

        Ok(tree)
    }

    pub async fn read_file(&self, path: impl AsRef<Path>) -> Result<Vec<u8>, FileError> {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .ok_or(FileError::NotFound(path.to_path_buf()))?;
        let dir = self.read_dir(path.parent()).await?;

        let entry = dir
            .entries
            .iter()
            .find(|e| e.filename.as_slice() == filename.as_encoded_bytes())
            .ok_or(FileError::NotFound(path.to_path_buf()))?;

        if !entry.mode.is_blob() {
            return Err(FileError::NotAFile(path.to_path_buf()));
        }

        Ok(self.repo.read_blob(entry.oid).await?.data)
    }
}

#[derive(Error, Debug)]
pub enum FileError {
    #[error("failed to read")]
    Read(#[from] ReadObjectError),

    #[error("may not be an absolute path")]
    AbsolutePath,

    #[error("not found: {0}")]
    NotFound(PathBuf),

    #[error("not a directory: {0}")]
    NotADirectory(PathBuf),

    #[error("not a file: {0}")]
    NotAFile(PathBuf),
}

impl Client {
    pub async fn deployment(
        &self,
        repo: &RepositoryName,
        deployment_id: &DeploymentId,
    ) -> Result<Deployment, DeploymentQueryError> {
        let pager = self
            .session
            .execute_iter(
                self.statements.find_deployment.clone(),
                (
                    repo.to_string(),
                    &deployment_id.ref_name,
                    deployment_id.commit_hash.as_slice(),
                ),
            )
            .await?;

        match pager.rows_stream::<(DateTime<Utc>, String)>()?.next().await {
            None => Err(DeploymentQueryError::NotFound),
            Some(Err(e)) => Err(e.into()),
            Some(Ok((created_at, state))) => Ok(Deployment {
                repository: repo.clone(),
                id: deployment_id.clone(),
                created_at,
                state: state.parse()?,
            }),
        }
    }

    pub async fn active_deployments(
        &self,
    ) -> Result<BoxStream<'static, Result<Deployment, DeploymentQueryError>>, DeploymentQueryError>
    {
        let pager = self
            .session
            .execute_iter(self.statements.list_active_deployments.clone(), ())
            .await?;

        let ids = pager
            .rows_stream::<(String,)>()?
            .map(|r| r.map(|r| r.0))
            .try_collect::<Vec<_>>()
            .await?;

        if ids.is_empty() {
            return Ok(stream::empty().boxed());
        }

        let deployments = self
            .session
            .execute_iter(self.statements.find_deployments_by_ids.clone(), (ids,))
            .await?;

        Ok(deployments
            .rows_stream::<(String, String, Vec<u8>, DateTime<Utc>, String)>()?
            .map(|r| {
                let (repository, ref_name, commit_hash, created_at, state) = r?;
                Ok::<_, DeploymentQueryError>(Deployment {
                    repository: repository.parse()?,
                    id: DeploymentId {
                        ref_name,
                        commit_hash: ObjectId::from_bytes_or_panic(&commit_hash),
                    },
                    created_at,
                    state: state.parse()?,
                })
            })
            .boxed())
    }
}

impl RepositoryClient {
    pub async fn active_deployments(
        &self,
    ) -> Result<BoxStream<'static, Result<Deployment, DeploymentQueryError>>, DeploymentQueryError>
    {
        let pager = self
            .client
            .session
            .execute_iter(
                self.client
                    .statements
                    .list_active_deployments_by_repo
                    .clone(),
                (self.name.to_string(),),
            )
            .await?;

        let ids = pager
            .rows_stream::<(String,)>()?
            .map(|r| r.map(|r| r.0))
            .try_collect::<Vec<_>>()
            .await?;

        if ids.is_empty() {
            return Ok(stream::empty().boxed());
        }

        let deployments = self
            .client
            .session
            .execute_iter(
                self.client.statements.find_deployments_by_ids.clone(),
                (ids,),
            )
            .await?;

        Ok(deployments
            .rows_stream::<(String, String, Vec<u8>, DateTime<Utc>, String)>()?
            .map(|r| {
                let (repository, ref_name, commit_hash, created_at, state) = r?;
                Ok::<_, DeploymentQueryError>(Deployment {
                    repository: repository.parse()?,
                    id: DeploymentId {
                        ref_name,
                        commit_hash: ObjectId::from_bytes_or_panic(&commit_hash),
                    },
                    created_at,
                    state: state.parse()?,
                })
            })
            .boxed())
    }

    pub async fn deployments(
        &self,
    ) -> Result<impl Stream<Item = Result<Deployment, DeploymentQueryError>>, DeploymentQueryError>
    {
        let pager = self
            .client
            .session
            .execute_iter(
                self.client.statements.list_deployments_by_repo.clone(),
                (self.name.to_string(),),
            )
            .await?;

        let repo = self.name.clone();
        Ok(pager
            .rows_stream::<(String, Vec<u8>, DateTime<Utc>, String)>()?
            .map(move |r| {
                let (ref_name, commit_hash, created_at, state) = r?;
                Ok::<_, DeploymentQueryError>(Deployment {
                    repository: repo.clone(),
                    id: DeploymentId {
                        ref_name,
                        commit_hash: ObjectId::from_bytes_or_panic(&commit_hash),
                    },
                    created_at,
                    state: state.parse()?,
                })
            }))
    }
}

#[derive(Error, Debug)]
pub enum DeploymentQueryError {
    #[error("failed to execute: {0}")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("failed to execute: {0}")]
    ScyllaExecution(#[from] ExecutionError),

    #[error("failed to parse row: {0}")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    ScyllaNextRow(#[from] NextRowError),

    #[error("failed to load row: {0}")]
    ScyllaIntoRows(#[from] IntoRowsResultError),

    #[error("failed to load single row: {0}")]
    ScyllaSingleRow(#[from] SingleRowError),

    #[error("{0}")]
    InvalidState(#[from] InvalidDeploymentState),

    #[error("{0}")]
    InvalidRepositoryName(#[from] InvalidRepositoryName),

    #[error("deployment not found")]
    NotFound,
}

impl Client {
    pub async fn set_deployment(&self, deployment: Deployment) -> Result<(), SetDeploymentError> {
        let deployment_id = deployment.fqid();
        let (dep, dep_by_id, active_dep) = futures::join!(
            self.session.execute_unpaged(
                &self.statements.set_deployment,
                (
                    deployment.repository.to_string(),
                    &deployment.id.ref_name,
                    deployment.id.commit_hash.as_slice(),
                    deployment.created_at,
                    deployment.state.to_string(),
                ),
            ),
            self.session.execute_unpaged(
                &self.statements.set_deployment_by_id,
                (
                    deployment_id.clone(),
                    deployment.repository.to_string(),
                    &deployment.id.ref_name,
                    deployment.id.commit_hash.as_slice(),
                    deployment.created_at,
                    deployment.state.to_string(),
                ),
            ),
            async {
                if deployment.state == DeploymentState::Down {
                    self.session
                        .execute_unpaged(
                            &self.statements.unset_active_deployment,
                            (
                                deployment.repository.to_string(),
                                &deployment.id.ref_name,
                                deployment.id.commit_hash.as_slice(),
                            ),
                        )
                        .await
                } else {
                    self.session
                        .execute_unpaged(
                            &self.statements.set_active_deployment,
                            (
                                deployment.repository.to_string(),
                                &deployment.id.ref_name,
                                deployment.id.commit_hash.as_slice(),
                                deployment_id.clone(),
                            ),
                        )
                        .await
                }
            }
        );

        dep?;
        dep_by_id?;
        active_dep?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum SetDeploymentError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("failed to query: {0}")]
    Query(#[from] DeploymentQueryError),
}

impl DeploymentClient {
    pub async fn mark_superceded_by(
        &self,
        commit_hash: ObjectId,
    ) -> Result<(), SetDeploymentError> {
        self.repo
            .client
            .session
            .execute_unpaged(
                &self.repo.client.statements.create_supercession,
                (
                    self.id.commit_hash.as_bytes(),
                    self.repo.name.to_string(),
                    &self.id.ref_name,
                    commit_hash.as_bytes(),
                ),
            )
            .await?;
        Ok(())
    }

    pub async fn get_superceded(&self) -> Result<Option<DeploymentClient>, DeploymentQueryError> {
        let r = self
            .repo
            .client
            .session
            .execute_unpaged(
                &self.repo.client.statements.get_superceded_commit,
                (
                    self.repo.name.to_string(),
                    &self.id.ref_name,
                    self.id.commit_hash.as_bytes(),
                ),
            )
            .await?;

        Ok(r.into_rows_result()?
            .single_row::<(Vec<u8>,)>()
            .ok()
            .map(|(superceded,)| {
                self.repo.deployment(DeploymentId {
                    ref_name: self.id.ref_name.clone(),
                    commit_hash: ObjectId::from_bytes_or_panic(&superceded),
                })
            }))
    }
}
