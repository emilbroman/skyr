use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::deployment::{Deployment, InvalidDeploymentState};
use crate::{DeploymentState, Repository};
use chrono::{DateTime, Utc};
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt, TryStreamExt, stream};
use gix_object::{Blob, Commit, Kind, Object, Tree, WriteTo};
use ids::{DeploymentId, DeploymentNonce, EnvironmentId, ObjId, ParseIdError, RepoQid};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::{
        ExecutionError, IntoRowsResultError, NewSessionError, NextRowError, PagerExecutionError,
        PrepareError, SingleRowError, TypeCheckError,
    },
    statement::prepared::PreparedStatement,
};
use thiserror::Error;

fn kind_to_db(kind: Kind) -> i8 {
    match kind {
        Kind::Commit => 1,
        Kind::Tree => 2,
        Kind::Blob => 3,
        Kind::Tag => 4,
    }
}

fn kind_from_db(kind: Option<i8>) -> Kind {
    match kind {
        Some(1) => Kind::Commit,
        Some(2) => Kind::Tree,
        Some(3) => Kind::Blob,
        Some(4) => Kind::Tag,
        // Legacy rows written before the kind column was added have NULL.
        // Default to Blob since it's the most permissive (content is opaque).
        _ => Kind::Blob,
    }
}

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
                nonce BIGINT,
                state TEXT,
                bootstrapped BOOLEAN,
                PRIMARY KEY ((organization, repository), created_at, environment_id, commit_hash, nonce)
            ) WITH CLUSTERING ORDER BY (created_at DESC, environment_id ASC, commit_hash ASC, nonce ASC)
        "#,

        create_deployments_by_id_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.deployments_by_id (
                deployment_qid TEXT,
                organization TEXT,
                repository TEXT,
                environment_id TEXT,
                commit_hash BLOB,
                nonce BIGINT,
                created_at TIMESTAMP,
                state TEXT,
                bootstrapped BOOLEAN,
                PRIMARY KEY ((deployment_qid))
            )
        "#,

        create_active_deployments_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.active_deployments (
                organization TEXT,
                repository TEXT,
                environment_id TEXT,
                commit_hash BLOB,
                nonce BIGINT,
                deployment_qid TEXT,
                PRIMARY KEY ((organization), repository, environment_id, commit_hash, nonce)
            )
        "#,

        create_objects_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.objects (
                organization TEXT,
                repository TEXT,
                hash BLOB,
                kind TINYINT,
                contents BLOB,
                PRIMARY KEY ((organization, repository), hash)
            )
        "#,

        create_supersessions_table = r#"
            CREATE TABLE IF NOT EXISTS cdb.supersessions (
                organization TEXT,
                repository TEXT,
                environment_id TEXT,
                superseding_commit_hash BLOB,
                superseding_nonce BIGINT,
                superseded_commit_hash BLOB,
                superseded_nonce BIGINT,
                PRIMARY KEY ((organization), repository, environment_id, superseded_commit_hash, superseded_nonce)
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
            SELECT kind, contents FROM cdb.objects
            WHERE organization = ?
            AND repository = ?
            AND hash = ?
        "#,

        write_object = r#"
            INSERT INTO cdb.objects (organization, repository, hash, kind, contents)
            VALUES (?, ?, ?, ?, ?)
        "#,

        find_deployment_by_qid = r#"
            SELECT organization, repository, environment_id, commit_hash, nonce, created_at, state, bootstrapped
            FROM cdb.deployments_by_id
            WHERE deployment_qid = ?
        "#,

        find_deployments_by_qids = r#"
            SELECT organization, repository, environment_id, commit_hash, nonce, created_at, state, bootstrapped
            FROM cdb.deployments_by_id
            WHERE deployment_qid IN ?
        "#,

        list_deployments_by_repo = r#"
            SELECT created_at, environment_id, commit_hash, nonce, state, bootstrapped
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
            INSERT INTO cdb.deployments (organization, repository, created_at, environment_id, commit_hash, nonce, state, bootstrapped)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        set_deployment_by_id = r#"
            INSERT INTO cdb.deployments_by_id (
                deployment_qid,
                organization,
                repository,
                environment_id,
                commit_hash,
                nonce,
                created_at,
                state,
                bootstrapped
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,

        set_active_deployment = r#"
            INSERT INTO cdb.active_deployments (organization, repository, environment_id, commit_hash, nonce, deployment_qid)
            VALUES (?, ?, ?, ?, ?, ?)
        "#,

        unset_active_deployment = r#"
            DELETE FROM cdb.active_deployments
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND commit_hash = ?
            AND nonce = ?
        "#,

        create_supersession = r#"
            INSERT INTO cdb.supersessions (
                superseding_commit_hash,
                superseding_nonce,
                organization,
                repository,
                environment_id,
                superseded_commit_hash,
                superseded_nonce
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
        "#,

        get_superseded_deployments = r#"
            SELECT superseded_commit_hash, superseded_nonce
            FROM cdb.supersessions
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND superseding_commit_hash = ?
            AND superseding_nonce = ?
            ALLOW FILTERING
        "#,

        get_superseding_deployment = r#"
            SELECT superseding_commit_hash, superseding_nonce
            FROM cdb.supersessions
            WHERE organization = ?
            AND repository = ?
            AND environment_id = ?
            AND superseded_commit_hash = ?
            AND superseded_nonce = ?
        "#,
    }
}

pub struct ClientBuilder {
    inner: SessionBuilder,
    replication_factor: u8,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            inner: SessionBuilder::default(),
            replication_factor: 1,
        }
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn known_node(mut self, hostname: impl AsRef<str>) -> Self {
        self.inner = self.inner.known_node(hostname);
        self
    }

    pub fn replication_factor(mut self, factor: u8) -> Self {
        self.replication_factor = factor;
        self
    }

    pub async fn build(&self) -> Result<Client, ConnectError> {
        let session = Arc::new(self.inner.build().await?);

        let create_keyspace_cql = format!(
            "CREATE KEYSPACE IF NOT EXISTS cdb \
             WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': {}}}",
            self.replication_factor,
        );
        let create_keyspace = session.prepare(create_keyspace_cql).await?;

        session.execute_unpaged(&create_keyspace, ()).await?;

        let statements = TableStatements::new(&session).await?;

        let (r0, r1, r2, r3, r4, r5) = futures::join!(
            session.execute_unpaged(&statements.create_repositories_table, ()),
            session.execute_unpaged(&statements.create_deployments_table, ()),
            session.execute_unpaged(&statements.create_active_deployments_table, ()),
            session.execute_unpaged(&statements.create_deployments_by_id_table, ()),
            session.execute_unpaged(&statements.create_objects_table, ()),
            session.execute_unpaged(&statements.create_supersessions_table, ()),
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

#[allow(clippy::too_many_arguments)]
fn deployment_from_row(
    organization: String,
    repository: String,
    environment_id: String,
    commit_hash: Vec<u8>,
    nonce: i64,
    created_at: DateTime<Utc>,
    state: String,
    bootstrapped: Option<bool>,
) -> Result<Deployment, DeploymentQueryError> {
    let commit = ObjId::from_bytes(&commit_hash).map_err(|_| DeploymentQueryError::NotFound)?;
    let org: ids::OrgId = organization.parse()?;
    let repo: ids::RepoId = repository.parse()?;
    let environment: EnvironmentId = environment_id.parse()?;
    Ok(Deployment {
        repo: RepoQid::new(org, repo),
        environment,
        deployment: DeploymentId::new(commit, DeploymentNonce::from_u64(nonce as u64)),
        created_at,
        state: state.parse()?,
        bootstrapped: bootstrapped.unwrap_or(false),
    })
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

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

    pub(crate) async fn set_repository(
        &self,
        repository: Repository,
    ) -> Result<(), ExecutionError> {
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
                let org: ids::OrgId = organization
                    .parse()
                    .map_err(RepositoryQueryError::InvalidId)?;
                let repo: ids::RepoId = repository
                    .parse()
                    .map_err(RepositoryQueryError::InvalidId)?;
                Ok::<_, RepositoryQueryError>(Repository {
                    name: RepoQid::new(org, repo),
                    created_at,
                })
            }))
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
            .rows_stream::<(
                String,
                String,
                String,
                Vec<u8>,
                i64,
                DateTime<Utc>,
                String,
                Option<bool>,
            )>()?
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
                nonce_val,
                created_at,
                state,
                bootstrapped,
            ))) => {
                if organization != repo.org.as_str() || repository != repo.repo.as_str() {
                    return Err(DeploymentQueryError::NotFound);
                }
                deployment_from_row(
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    nonce_val,
                    created_at,
                    state,
                    bootstrapped,
                )
            }
        }
    }

    pub(crate) async fn set_deployment(
        &self,
        deployment: Deployment,
    ) -> Result<(), SetDeploymentError> {
        let deployment_qid = deployment.deployment_qid().to_string();
        let repo = &deployment.repo;
        let commit_hash = deployment.deployment.commit.as_bytes();
        let nonce_val = deployment.deployment.nonce.as_u64() as i64;
        let (dep, dep_by_id, active_dep) = futures::join!(
            self.session.execute_unpaged(
                &self.statements.set_deployment,
                (
                    repo.org.as_str(),
                    repo.repo.as_str(),
                    deployment.created_at,
                    deployment.environment.as_str(),
                    commit_hash,
                    nonce_val,
                    deployment.state.to_string(),
                    deployment.bootstrapped,
                ),
            ),
            self.session.execute_unpaged(
                &self.statements.set_deployment_by_id,
                (
                    deployment_qid.as_str(),
                    repo.org.as_str(),
                    repo.repo.as_str(),
                    deployment.environment.as_str(),
                    commit_hash,
                    nonce_val,
                    deployment.created_at,
                    deployment.state.to_string(),
                    deployment.bootstrapped,
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
                                commit_hash,
                                nonce_val,
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
                                commit_hash,
                                nonce_val,
                                deployment_qid.as_str(),
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
            .rows_stream::<(
                String,
                String,
                String,
                Vec<u8>,
                i64,
                DateTime<Utc>,
                String,
                Option<bool>,
            )>()?
            .map(|r| {
                let (
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    nonce,
                    created_at,
                    state,
                    bootstrapped,
                ) = r?;
                deployment_from_row(
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    nonce,
                    created_at,
                    state,
                    bootstrapped,
                )
            })
            .boxed())
    }
}

// ---------------------------------------------------------------------------
// RepositoryClient
// ---------------------------------------------------------------------------

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

    pub fn commit(&self, commit: ObjId) -> CommitClient {
        CommitClient {
            repo: self.clone(),
            commit,
        }
    }

    pub fn deployment(
        &self,
        environment: EnvironmentId,
        deployment: DeploymentId,
    ) -> DeploymentClient {
        let commit_client = self.commit(deployment.commit);
        DeploymentClient {
            commit_client,
            environment,
            deployment,
        }
    }

    async fn read_object(&self, hash: ObjId) -> Result<(Kind, Vec<u8>), LoadObjectError> {
        let repo = &self.name;
        let pager = self
            .client
            .session
            .execute_iter(
                self.client.statements.read_object.clone(),
                (repo.org.as_str(), repo.repo.as_str(), hash.as_bytes()),
            )
            .await?;

        match pager
            .rows_stream::<(Option<i8>, Vec<u8>)>()?
            .try_next()
            .await?
        {
            None => Err(LoadObjectError::NotFound),
            Some((kind, contents)) => {
                let kind = kind_from_db(kind);
                Ok((kind, contents))
            }
        }
    }

    pub async fn write_object(&self, id: ObjId, object: Object) -> Result<(), WriteObjectError> {
        let kind = kind_to_db(object.kind());
        let mut data = vec![];
        object.write_to(&mut data)?;
        let repo = &self.name;

        self.client
            .session
            .execute_unpaged(
                &self.client.statements.write_object,
                (
                    repo.org.as_str(),
                    repo.repo.as_str(),
                    id.as_bytes(),
                    kind,
                    data,
                ),
            )
            .await?;

        Ok(())
    }

    pub async fn read_raw_object(&self, hash: ObjId) -> Result<(Kind, Vec<u8>), LoadObjectError> {
        self.read_object(hash).await
    }

    pub async fn write_commit(&self, id: ObjId, object: Commit) -> Result<(), WriteObjectError> {
        self.write_object(id, Object::Commit(object)).await
    }

    pub async fn read_commit(&self, hash: ObjId) -> Result<Commit, ReadObjectError> {
        let (_, data) = self.read_object(hash).await?;
        let commit = gix_object::CommitRef::from_bytes(&data)?;
        Ok(commit.into_owned()?)
    }

    pub async fn write_tree(&self, id: ObjId, object: Tree) -> Result<(), WriteObjectError> {
        self.write_object(id, Object::Tree(object)).await
    }

    pub async fn read_tree(&self, hash: ObjId) -> Result<Tree, ReadObjectError> {
        let (_, data) = self.read_object(hash).await?;
        let tree = gix_object::TreeRef::from_bytes(&data)?;
        Ok(tree.into_owned())
    }

    pub async fn write_blob(&self, id: ObjId, object: Blob) -> Result<(), WriteObjectError> {
        self.write_object(id, Object::Blob(object)).await
    }

    pub async fn read_blob(&self, hash: ObjId) -> Result<Blob, ReadObjectError> {
        let (_, data) = self.read_object(hash).await?;
        // BlobRef::from_bytes is infallible (returns Result<_, Infallible>).
        let Ok(blob) = gix_object::BlobRef::from_bytes(&data);
        Ok(blob.into_owned())
    }

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
            .rows_stream::<(
                String,
                String,
                String,
                Vec<u8>,
                i64,
                DateTime<Utc>,
                String,
                Option<bool>,
            )>()?
            .map(|r| {
                let (
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    nonce,
                    created_at,
                    state,
                    bootstrapped,
                ) = r?;
                deployment_from_row(
                    organization,
                    repository,
                    environment_id,
                    commit_hash,
                    nonce,
                    created_at,
                    state,
                    bootstrapped,
                )
            })
            .boxed())
    }

    /// Returns the current deployment for the given environment, if any.
    ///
    /// "Current" is defined by supersession: among the non-`Down`
    /// deployments in the environment, the one that has not been
    /// superseded by another deployment. There is at most one such
    /// deployment per environment, since [`DeploymentClient::make_desired`]
    /// records a supersession row for the previously-current deployment
    /// before promoting the new one.
    pub async fn current_deployment(
        &self,
        environment: &EnvironmentId,
    ) -> Result<Option<Deployment>, DeploymentQueryError> {
        let mut stream = self.active_deployments().await?;
        let mut candidates: Vec<Deployment> = Vec::new();
        while let Some(dep) = stream.next().await {
            let dep = dep?;
            if dep.environment == *environment {
                candidates.push(dep);
            }
        }

        for dep in candidates {
            let dc = self.deployment(dep.environment.clone(), dep.deployment.clone());
            if dc.get_superseding().await?.is_none() {
                return Ok(Some(dep));
            }
        }
        Ok(None)
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
            .rows_stream::<(DateTime<Utc>, String, Vec<u8>, i64, String, Option<bool>)>()?
            .map(move |r| {
                let (created_at, environment_id, commit_hash, nonce, state, bootstrapped) = r?;
                deployment_from_row(
                    repo.org.to_string(),
                    repo.repo.to_string(),
                    environment_id,
                    commit_hash,
                    nonce,
                    created_at,
                    state,
                    bootstrapped,
                )
            }))
    }
}

// ---------------------------------------------------------------------------
// CommitClient
// ---------------------------------------------------------------------------

/// A client scoped to a single commit within a repository.
///
/// Provides commit-bound Git object reads (tree walks, file reads). The client
/// does not know — and does not need to know — whether the commit is part of
/// any deployment.
#[derive(Clone)]
pub struct CommitClient {
    repo: RepositoryClient,
    commit: ObjId,
}

impl CommitClient {
    pub fn repo_qid(&self) -> &RepoQid {
        &self.repo.name
    }

    pub fn commit_hash(&self) -> ObjId {
        self.commit
    }

    pub async fn read_commit(&self) -> Result<Commit, ReadObjectError> {
        self.repo.read_commit(self.commit_hash()).await
    }

    pub async fn read_dir(&self, path: Option<impl AsRef<Path>>) -> Result<Tree, FileError> {
        let commit = self.repo.read_commit(self.commit_hash()).await?;
        let mut tree = self.repo.read_tree(commit.tree.into()).await?;

        let mut result_buf = PathBuf::new();

        if let Some(path) = path {
            for segment in path.as_ref() {
                if segment == "." {
                    continue;
                }

                if segment == ".." {
                    return Err(FileError::NotFound(path.as_ref().to_path_buf()));
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

                        tree = self.repo.read_tree(entry.oid.into()).await?;
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

        Ok(self.repo.read_blob(entry.oid.into()).await?.data)
    }

    /// Return the Git object hash (blob or tree OID) for a path, or `None`
    /// if the path does not exist.
    pub async fn path_hash(&self, path: impl AsRef<Path>) -> Result<Option<ObjId>, FileError> {
        let path = path.as_ref();

        // Root path → return the commit's tree OID.
        if path.as_os_str().is_empty() {
            let commit = self.repo.read_commit(self.commit_hash()).await?;
            return Ok(Some(commit.tree.into()));
        }

        let filename = match path.file_name() {
            Some(f) => f,
            None => return Ok(None),
        };

        let dir = match self.read_dir(path.parent()).await {
            Ok(tree) => tree,
            Err(FileError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };

        Ok(dir
            .entries
            .iter()
            .find(|e| e.filename.as_slice() == filename.as_encoded_bytes())
            .map(|e| e.oid.into()))
    }
}

// ---------------------------------------------------------------------------
// DeploymentClient
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct DeploymentClient {
    commit_client: CommitClient,
    environment: EnvironmentId,
    deployment: DeploymentId,
}

impl std::ops::Deref for DeploymentClient {
    type Target = CommitClient;
    fn deref(&self) -> &CommitClient {
        &self.commit_client
    }
}

impl DeploymentClient {
    /// Borrow the underlying commit-scoped client.
    pub fn commit_client(&self) -> &CommitClient {
        &self.commit_client
    }

    /// Discard the deployment-level scope and return an owned commit-scoped
    /// client.
    pub fn into_commit_client(self) -> CommitClient {
        self.commit_client
    }

    pub fn deployment_qid(&self) -> ids::DeploymentQid {
        self.commit_client
            .repo
            .name
            .environment(self.environment.clone())
            .deployment(self.deployment.clone())
    }

    pub fn environment_qid(&self) -> ids::EnvironmentQid {
        self.commit_client
            .repo
            .name
            .environment(self.environment.clone())
    }

    pub async fn get(&self) -> Result<Deployment, DeploymentQueryError> {
        self.commit_client
            .repo
            .client
            .find_deployment(
                &self.commit_client.repo.name,
                &self.environment,
                &self.deployment,
            )
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
            return Ok(());
        }

        let deployment = Deployment {
            repo: self.commit_client.repo.name.clone(),
            environment: self.environment.clone(),
            deployment: self.deployment.clone(),
            created_at: prev_state
                .as_ref()
                .map(|s| s.created_at)
                .unwrap_or_else(Utc::now),
            state,
            bootstrapped: prev_state.as_ref().map(|s| s.bootstrapped).unwrap_or(false),
        };

        self.commit_client
            .repo
            .client
            .set_deployment(deployment)
            .await?;

        Ok(())
    }

    /// Update the bootstrapped flag without changing the state.
    pub async fn set_progress(&self, bootstrapped: bool) -> Result<(), SetDeploymentError> {
        let prev = self.get().await?;

        let deployment = Deployment {
            bootstrapped,
            ..prev
        };

        self.commit_client
            .repo
            .client
            .set_deployment(deployment)
            .await?;

        Ok(())
    }

    pub async fn mark_superseded_by(
        &self,
        superseding: &DeploymentId,
    ) -> Result<(), SetDeploymentError> {
        let superseding_oid = superseding.commit.as_bytes();
        let this_oid = self.commit_client.commit_hash();
        let repo = &self.commit_client.repo;
        repo.client
            .session
            .execute_unpaged(
                &repo.client.statements.create_supersession,
                (
                    superseding_oid,
                    superseding.nonce.as_u64() as i64,
                    repo.name.org.as_str(),
                    repo.name.repo.as_str(),
                    self.environment.as_str(),
                    this_oid.as_bytes(),
                    self.deployment.nonce.as_u64() as i64,
                ),
            )
            .await?;
        Ok(())
    }

    pub async fn superseded(&self) -> Result<Vec<DeploymentClient>, DeploymentQueryError> {
        let this_oid = self.commit_client.commit_hash();
        let repo = &self.commit_client.repo;
        let pager = repo
            .client
            .session
            .execute_iter(
                repo.client.statements.get_superseded_deployments.clone(),
                (
                    repo.name.org.as_str(),
                    repo.name.repo.as_str(),
                    self.environment.as_str(),
                    this_oid.as_bytes(),
                    self.deployment.nonce.as_u64() as i64,
                ),
            )
            .await?;

        let superseded = pager
            .rows_stream::<(Vec<u8>, i64)>()?
            .map(|row| {
                let (commit_hash, nonce) = row?;
                let commit =
                    ObjId::from_bytes(&commit_hash).map_err(|_| DeploymentQueryError::NotFound)?;
                let deploy_id = DeploymentId::new(commit, DeploymentNonce::from_u64(nonce as u64));
                Ok::<_, DeploymentQueryError>(repo.deployment(self.environment.clone(), deploy_id))
            })
            .try_collect::<Vec<_>>()
            .await?;

        Ok(superseded)
    }

    /// Transition this deployment into the `Desired` state, taking over
    /// from whatever deployment is currently active in the same
    /// environment.
    ///
    /// Any currently-active (`Desired`) deployment in this environment is
    /// transitioned to `Lingering` and a supersession row is recorded
    /// linking it to this deployment.
    pub async fn make_desired(&self) -> Result<(), SetDeploymentError> {
        let repo = &self.commit_client.repo;
        // Find currently-active deployments in the same environment.
        let mut active: Vec<Deployment> = Vec::new();
        let mut stream = repo.active_deployments().await?;
        while let Some(dep) = stream.next().await {
            let dep = dep?;
            if dep.environment != self.environment {
                continue;
            }
            if dep.deployment == self.deployment {
                continue;
            }
            if dep.state.is_active() {
                active.push(dep);
            }
        }

        // Mark each as Lingering and record the supersession.
        for dep in active {
            let predecessor = repo.deployment(dep.environment.clone(), dep.deployment.clone());
            let (r1, r2) = futures::join!(
                predecessor.set(DeploymentState::Lingering),
                predecessor.mark_superseded_by(&self.deployment),
            );
            r1?;
            r2?;
        }

        // Promote self.
        self.set(DeploymentState::Desired).await
    }

    pub async fn get_superseding(&self) -> Result<Option<DeploymentClient>, DeploymentQueryError> {
        let this_oid = self.commit_client.commit_hash();
        let repo = &self.commit_client.repo;
        let r = repo
            .client
            .session
            .execute_unpaged(
                &repo.client.statements.get_superseding_deployment,
                (
                    repo.name.org.as_str(),
                    repo.name.repo.as_str(),
                    self.environment.as_str(),
                    this_oid.as_bytes(),
                    self.deployment.nonce.as_u64() as i64,
                ),
            )
            .await?;

        Ok(r.into_rows_result()?
            .single_row::<(Vec<u8>, i64)>()
            .ok()
            .and_then(|(superseding, nonce)| {
                let commit = ObjId::from_bytes(&superseding).ok()?;
                let deploy_id = DeploymentId::new(commit, DeploymentNonce::from_u64(nonce as u64));
                Some(repo.deployment(self.environment.clone(), deploy_id))
            }))
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum LoadObjectError {
    #[error("not found")]
    NotFound,

    #[error("database query failed")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("database query failed")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("database query failed")]
    ScyllaNextRow(#[from] NextRowError),
}

#[derive(Error, Debug)]
pub enum ReadObjectError {
    #[error("failed to read object")]
    Read(#[from] LoadObjectError),

    #[error("failed to decode object")]
    Decode(#[from] gix_object::decode::Error),
}

#[derive(Error, Debug)]
pub enum WriteObjectError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Error, Debug)]
pub enum FileError {
    #[error("failed to read")]
    Read(#[from] ReadObjectError),

    #[error("not found")]
    NotFound(PathBuf),

    #[error("not a directory")]
    NotADirectory(PathBuf),

    #[error("not a file")]
    NotAFile(PathBuf),
}

#[derive(Error, Debug)]
pub enum RepositoryQueryError {
    #[error("failed to execute: {0}")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("failed to parse row: {0}")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    ScyllaNextRow(#[from] NextRowError),

    #[error("invalid ID in database: {0}")]
    InvalidId(ParseIdError),

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

    #[error("invalid ID in database: {0}")]
    InvalidId(#[from] ParseIdError),

    #[error("deployment not found")]
    NotFound,
}

#[derive(Error, Debug)]
pub enum SetDeploymentError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("failed to query: {0}")]
    Query(#[from] DeploymentQueryError),
}
