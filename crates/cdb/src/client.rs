use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::deployment::{Deployment, InvalidDeploymentState};
use crate::{DeploymentState, Repository};
use chrono::{DateTime, Utc};
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt, TryStreamExt, stream};
use gix_hash::ObjectId;
use gix_object::{Blob, Commit, Object, Tree, WriteTo};
use ids::{DeploymentId, EnvironmentId, RepoQid};
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
        create_repositories_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.repositories (
                organization TEXT,
                repository TEXT,
                created_at TIMESTAMP,
                PRIMARY KEY ((organization), repository)
            )
        "#,

        create_deployments_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.deployments (
                organization TEXT,
                repository TEXT,
                created_at TIMESTAMP,
                environment_id TEXT,
                commit_hash BLOB,
                state TEXT,
                PRIMARY KEY ((organization, repository), created_at, environment_id, commit_hash)
            ) WITH CLUSTERING ORDER BY (created_at DESC, environment_id ASC, commit_hash ASC)
        "#,

        create_deployments_by_id_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.deployments_by_id (
                deployment_qid TEXT,
                organization TEXT,
                repository TEXT,
                environment_id TEXT,
                commit_hash BLOB,
                created_at TIMESTAMP,
                state TEXT,
                PRIMARY KEY ((deployment_qid))
            )
        "#,

        create_active_deployments_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.active_deployments (
                organization TEXT,
                repository TEXT,
                environment_id TEXT,
                commit_hash BLOB,
                deployment_qid TEXT,
                PRIMARY KEY ((organization), repository, environment_id, commit_hash)
            )
        "#,

        create_objects_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.objects (
                organization TEXT,
                repository TEXT,
                hash BLOB,
                contents BLOB,
                PRIMARY KEY ((organization, repository), hash)
            )
        "#,

        create_supercessions_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.supercessions (
                organization TEXT,
                repository TEXT,
                environment_id TEXT,
                superceding_commit_hash BLOB,
                superceded_commit_hash BLOB,
                PRIMARY KEY ((organization), repository, environment_id, superceded_commit_hash)
            )
        "#,
    }

    PreparedStatements {
        find_repository = r#"
            SELECT created_at
            FROM cdb.repositories
            WHERE organization = ?
            AND repository = ?
        "#,

        set_repository = r#"
            INSERT INTO cdb.repositories (organization, repository, created_at)
            VALUES (?, ?, ?)
        "#,

        read_object = r#"
            SELECT contents FROM cdb.objects
            WHERE organization = ?
            AND repository = ?
            AND hash = ?
        "#,

        write_object = r#"
            INSERT INTO cdb.objects (organization, repository, hash, contents)
            VALUES (?, ?, ?, ?)
        "#,

        find_deployment_by_qid = r#"
            SELECT organization, repository, environment_id, commit_hash, created_at, state
            FROM cdb.deployments_by_id
            WHERE deployment_qid = ?
        "#,

        find_deployments_by_qids = r#"
            SELECT organization, repository, environment_id, commit_hash, created_at, state
            FROM cdb.deployments_by_id
            WHERE deployment_qid IN ?
        "#,

        list_deployments_by_repo = r#"
            SELECT created_at, environment_id, commit_hash, state
            FROM cdb.deployments
            WHERE organization = ?
            AND repository = ?
            ORDER BY created_at DESC
        "#,

        list_active_deployments = r#"
            SELECT deployment_qid
            FROM cdb.active_deployments
        "#,

        list_repositories_by_org = r#"
            SELECT repository, created_at
            FROM cdb.repositories
            WHERE organization = ?
            ORDER BY repository ASC
        "#,

        list_active_deployments_by_repo = r#"
            SELECT deployment_qid
            FROM cdb.active_deployments
            WHERE organization = ?
            AND repository = ?
        "#,

        set_deployment = r#"
            INSERT INTO cdb.deployments (organization, repository, created_at, environment_id, commit_hash, state)
            VALUES (?, ?, ?, ?, ?, ?)
        "#,

        set_deployment_by_id = r#"
            INSERT INTO cdb.deployments_by_id (
                deployment_qid,
                organization,
                repository,
                environment_id,
                commit_hash,
                created_at,
                state
            )
            VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,

        set_active_deployment = r#"
            INSERT INTO cdb.active_deployments (organization, repository, environment_id, commit_hash, deployment_qid)
            VALUES (?, ?, ?, ?, ?)
        "#,

        unset_active_deployment = r#"
            DELETE FROM cdb.active_deployments
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND commit_hash = ?
        "#,

        create_supercession = r#"
            UPDATE cdb.supercessions
            SET superceding_commit_hash = ?
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND superceded_commit_hash = ?
        "#,

        get_superceded_commits = r#"
            SELECT superceded_commit_hash
            FROM cdb.supercessions
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND superceding_commit_hash = ?
            ALLOW FILTERING
        "#,

        get_superceding_commit = r#"
            SELECT superceding_commit_hash
            FROM cdb.supercessions
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND superceded_commit_hash = ?
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

        let (r0, r1, r2, r3, r4, r5) = futures::join!(
            session.execute_unpaged(&statements.create_repositories_table, ()),
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
        r5?;

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
    pub fn repo(&self, name: RepoQid) -> RepositoryClient {
        RepositoryClient {
            client: self.clone(),
            name,
        }
    }
}

#[derive(Clone)]
pub struct RepositoryClient {
    client: Client,
    name: RepoQid,
}

impl RepositoryClient {
    pub fn repo_qid(&self) -> &RepoQid {
        &self.name
    }

    pub async fn get(&self) -> Result<Repository, RepositoryQueryError> {
        self.client.repository(&self.name).await
    }

    pub async fn create(&self) -> Result<Repository, CreateRepositoryError> {
        match self.get().await {
            Ok(_) => Err(CreateRepositoryError::AlreadyExists),
            Err(RepositoryQueryError::NotFound) => {
                let repository = Repository {
                    name: self.name.clone(),
                    created_at: Utc::now(),
                };
                self.client.set_repository(repository.clone()).await?;
                Ok(repository)
            }
            Err(e) => Err(CreateRepositoryError::Query(e)),
        }
    }

    pub fn deployment(
        &self,
        environment: EnvironmentId,
        deployment: DeploymentId,
    ) -> DeploymentClient {
        DeploymentClient {
            repo: self.clone(),
            environment,
            deployment,
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
        let repo = &self.name;
        let pager = self
            .client
            .session
            .execute_iter(
                self.client.statements.read_object.clone(),
                (repo.org.as_str(), repo.repo.as_str(), hash.as_bytes()),
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
        let repo = &self.name;

        self.client
            .session
            .execute_unpaged(
                &self.client.statements.write_object,
                (repo.org.as_str(), repo.repo.as_str(), id.as_slice(), data),
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

/// Client for interacting with a specific deployment (environment + commit).
#[derive(Clone)]
pub struct DeploymentClient {
    repo: RepositoryClient,
    environment: EnvironmentId,
    deployment: DeploymentId,
}

impl DeploymentClient {
    /// Returns the repository QID for this deployment.
    pub fn repo_qid(&self) -> &RepoQid {
        &self.repo.name
    }

    /// Returns the environment ID for this deployment.
    pub fn environment_id(&self) -> &EnvironmentId {
        &self.environment
    }

    /// Returns the deployment ID (commit hash) for this deployment.
    pub fn deployment_id(&self) -> &DeploymentId {
        &self.deployment
    }

    /// Returns the fully qualified deployment identifier.
    pub fn deployment_qid(&self) -> ids::DeploymentQid {
        self.repo
            .name
            .environment(self.environment.clone())
            .deployment(self.deployment.clone())
    }

    /// Returns the fully qualified environment identifier.
    pub fn environment_qid(&self) -> ids::EnvironmentQid {
        self.repo.name.environment(self.environment.clone())
    }

    /// Returns the commit hash as a `gix_hash::ObjectId`.
    pub fn commit_hash(&self) -> ObjectId {
        ObjectId::from_bytes_or_panic(&self.deployment.to_bytes())
    }

    pub async fn get(&self) -> Result<Deployment, DeploymentQueryError> {
        self.repo
            .client
            .find_deployment(&self.repo.name, &self.environment, &self.deployment)
            .await
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
            repo: self.repo.name.clone(),
            environment: self.environment.clone(),
            deployment: self.deployment.clone(),
            created_at: prev_state
                .as_ref()
                .map(|s| s.created_at)
                .unwrap_or_else(Utc::now),
            state,
        };

        self.repo.client.set_deployment(deployment).await?;

        Ok(())
    }

    pub async fn read_dir(&self, path: Option<impl AsRef<Path>>) -> Result<Tree, FileError> {
        let commit = self.repo.read_commit(self.commit_hash()).await?;
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

/// Helper to convert a `DeploymentId` (40-char hex) to `gix_hash::ObjectId`.
fn deployment_id_to_oid(id: &DeploymentId) -> ObjectId {
    ObjectId::from_bytes_or_panic(&id.to_bytes())
}

/// Helper to construct a `Deployment` from raw DB row values.
fn deployment_from_row(
    organization: String,
    repository: String,
    environment_id: String,
    commit_hash: Vec<u8>,
    created_at: DateTime<Utc>,
    state: String,
) -> Result<Deployment, DeploymentQueryError> {
    let deploy_id =
        DeploymentId::from_bytes(&commit_hash).map_err(|_| DeploymentQueryError::NotFound)?;
    Ok(Deployment {
        repo: RepoQid::new(
            ids::OrgId::new_unchecked(organization),
            ids::RepoId::new_unchecked(repository),
        ),
        environment: EnvironmentId::new_unchecked(environment_id),
        deployment: deploy_id,
        created_at,
        state: state.parse()?,
    })
}

impl Client {
    pub async fn set_repository(&self, repository: Repository) -> Result<(), ExecutionError> {
        self.session
            .execute_unpaged(
                &self.statements.set_repository,
                (
                    repository.name.org.as_str(),
                    repository.name.repo.as_str(),
                    repository.created_at,
                ),
            )
            .await?;
        Ok(())
    }

    pub async fn repository(&self, name: &RepoQid) -> Result<Repository, RepositoryQueryError> {
        let pager = self
            .session
            .execute_iter(
                self.statements.find_repository.clone(),
                (name.org.as_str(), name.repo.as_str()),
            )
            .await?;

        match pager.rows_stream::<(DateTime<Utc>,)>()?.next().await {
            None => Err(RepositoryQueryError::NotFound),
            Some(Err(e)) => Err(e.into()),
            Some(Ok((created_at,))) => Ok(Repository {
                name: name.clone(),
                created_at,
            }),
        }
    }

    pub async fn find_deployment(
        &self,
        repo: &RepoQid,
        environment: &EnvironmentId,
        deployment: &DeploymentId,
    ) -> Result<Deployment, DeploymentQueryError> {
        let qid = repo
            .environment(environment.clone())
            .deployment(deployment.clone())
            .to_string();
        let pager = self
            .session
            .execute_iter(self.statements.find_deployment_by_qid.clone(), (qid,))
            .await?;

        match pager
            .rows_stream::<(String, String, String, Vec<u8>, DateTime<Utc>, String)>()?
            .next()
            .await
        {
            None => Err(DeploymentQueryError::NotFound),
            Some(Err(e)) => Err(e.into()),
            Some(Ok((
                organization,
                repository,
                environment_id,
                commit_hash,
                created_at,
                state,
            ))) => {
                if organization != repo.org.as_str() || repository != repo.repo.as_str() {
                    return Err(DeploymentQueryError::NotFound);
                }
                deployment_from_row(
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    created_at,
                    state,
                )
            }
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

        let qids = pager
            .rows_stream::<(String,)>()?
            .map(|r| r.map(|r| r.0))
            .try_collect::<Vec<_>>()
            .await?;

        if qids.is_empty() {
            return Ok(stream::empty().boxed());
        }

        let deployments = self
            .session
            .execute_iter(self.statements.find_deployments_by_qids.clone(), (qids,))
            .await?;

        Ok(deployments
            .rows_stream::<(String, String, String, Vec<u8>, DateTime<Utc>, String)>()?
            .map(|r| {
                let (organization, repository, environment_id, commit_hash, created_at, state) = r?;
                deployment_from_row(
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    created_at,
                    state,
                )
            })
            .boxed())
    }

    pub async fn repositories_by_organization(
        &self,
        organization: impl Into<String>,
    ) -> Result<impl Stream<Item = Result<Repository, RepositoryQueryError>>, RepositoryQueryError>
    {
        let organization = organization.into();
        let pager = self
            .session
            .execute_iter(
                self.statements.list_repositories_by_org.clone(),
                (organization.as_str(),),
            )
            .await?;

        Ok(pager
            .rows_stream::<(String, DateTime<Utc>)>()?
            .map(move |row| {
                let (repository, created_at) = row?;
                Ok::<_, RepositoryQueryError>(Repository {
                    name: RepoQid::new(
                        ids::OrgId::new_unchecked(organization.clone()),
                        ids::RepoId::new_unchecked(repository),
                    ),
                    created_at,
                })
            }))
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
                (self.name.org.as_str(), self.name.repo.as_str()),
            )
            .await?;

        let qids = pager
            .rows_stream::<(String,)>()?
            .map(|r| r.map(|r| r.0))
            .try_collect::<Vec<_>>()
            .await?;

        if qids.is_empty() {
            return Ok(stream::empty().boxed());
        }

        let deployments = self
            .client
            .session
            .execute_iter(
                self.client.statements.find_deployments_by_qids.clone(),
                (qids,),
            )
            .await?;

        Ok(deployments
            .rows_stream::<(String, String, String, Vec<u8>, DateTime<Utc>, String)>()?
            .map(|r| {
                let (organization, repository, environment_id, commit_hash, created_at, state) = r?;
                deployment_from_row(
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    created_at,
                    state,
                )
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
                (self.name.org.as_str(), self.name.repo.as_str()),
            )
            .await?;

        let repo = self.name.clone();
        Ok(pager
            .rows_stream::<(DateTime<Utc>, String, Vec<u8>, String)>()?
            .map(move |r| {
                let (created_at, environment_id, commit_hash, state) = r?;
                let deploy_id = DeploymentId::from_bytes(&commit_hash)
                    .map_err(|_| DeploymentQueryError::NotFound)?;
                Ok::<_, DeploymentQueryError>(Deployment {
                    repo: repo.clone(),
                    environment: EnvironmentId::new_unchecked(environment_id),
                    deployment: deploy_id,
                    created_at,
                    state: state.parse()?,
                })
            }))
    }
}

#[derive(Error, Debug)]
pub enum RepositoryQueryError {
    #[error("failed to execute: {0}")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("failed to parse row: {0}")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    ScyllaNextRow(#[from] NextRowError),

    #[error("repository not found")]
    NotFound,
}

#[derive(Error, Debug)]
pub enum CreateRepositoryError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("failed to query repository: {0}")]
    Query(RepositoryQueryError),

    #[error("repository already exists")]
    AlreadyExists,
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

    #[error("deployment not found")]
    NotFound,
}

impl Client {
    pub async fn set_deployment(&self, deployment: Deployment) -> Result<(), SetDeploymentError> {
        let deployment_qid = deployment.deployment_qid().to_string();
        let repo = &deployment.repo;
        let commit_hash_bytes = deployment_id_to_oid(&deployment.deployment);
        let (dep, dep_by_id, active_dep) = futures::join!(
            self.session.execute_unpaged(
                &self.statements.set_deployment,
                (
                    repo.org.as_str(),
                    repo.repo.as_str(),
                    deployment.created_at,
                    deployment.environment.as_str(),
                    commit_hash_bytes.as_slice(),
                    deployment.state.to_string(),
                ),
            ),
            self.session.execute_unpaged(
                &self.statements.set_deployment_by_id,
                (
                    deployment_qid.clone(),
                    repo.org.as_str(),
                    repo.repo.as_str(),
                    deployment.environment.as_str(),
                    commit_hash_bytes.as_slice(),
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
                                repo.org.as_str(),
                                repo.repo.as_str(),
                                deployment.environment.as_str(),
                                commit_hash_bytes.as_slice(),
                            ),
                        )
                        .await
                } else {
                    self.session
                        .execute_unpaged(
                            &self.statements.set_active_deployment,
                            (
                                repo.org.as_str(),
                                repo.repo.as_str(),
                                deployment.environment.as_str(),
                                commit_hash_bytes.as_slice(),
                                deployment_qid.clone(),
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
        superceding_commit: &DeploymentId,
    ) -> Result<(), SetDeploymentError> {
        let superceding_oid = deployment_id_to_oid(superceding_commit);
        let this_oid = self.commit_hash();
        self.repo
            .client
            .session
            .execute_unpaged(
                &self.repo.client.statements.create_supercession,
                (
                    superceding_oid.as_bytes(),
                    self.repo.name.org.as_str(),
                    self.repo.name.repo.as_str(),
                    self.environment.as_str(),
                    this_oid.as_bytes(),
                ),
            )
            .await?;
        Ok(())
    }

    pub async fn superceded(&self) -> Result<Vec<DeploymentClient>, DeploymentQueryError> {
        let this_oid = self.commit_hash();
        let pager = self
            .repo
            .client
            .session
            .execute_iter(
                self.repo.client.statements.get_superceded_commits.clone(),
                (
                    self.repo.name.org.as_str(),
                    self.repo.name.repo.as_str(),
                    self.environment.as_str(),
                    this_oid.as_bytes(),
                ),
            )
            .await?;

        let superceded = pager
            .rows_stream::<(Vec<u8>,)>()?
            .map(|row| {
                let (commit_hash,) = row?;
                let deploy_id = DeploymentId::from_bytes(&commit_hash)
                    .map_err(|_| DeploymentQueryError::NotFound)?;
                Ok::<_, DeploymentQueryError>(
                    self.repo.deployment(self.environment.clone(), deploy_id),
                )
            })
            .try_collect::<Vec<_>>()
            .await?;

        Ok(superceded)
    }

    pub async fn get_superceding(&self) -> Result<Option<DeploymentClient>, DeploymentQueryError> {
        let this_oid = self.commit_hash();
        let r = self
            .repo
            .client
            .session
            .execute_unpaged(
                &self.repo.client.statements.get_superceding_commit,
                (
                    self.repo.name.org.as_str(),
                    self.repo.name.repo.as_str(),
                    self.environment.as_str(),
                    this_oid.as_bytes(),
                ),
            )
            .await?;

        Ok(r.into_rows_result()?
            .single_row::<(Vec<u8>,)>()
            .ok()
            .and_then(|(superceding,)| {
                let deploy_id = DeploymentId::from_bytes(&superceding).ok()?;
                Some(self.repo.deployment(self.environment.clone(), deploy_id))
            }))
    }
}
