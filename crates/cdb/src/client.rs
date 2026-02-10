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
use futures_util::{Stream, StreamExt, TryStreamExt};
use gix_hash::ObjectId;
use gix_object::{Blob, Commit, Object, Tree, WriteTo};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::{
        ExecutionError, NewSessionError, NextRowError, PagerExecutionError, PrepareError,
        TypeCheckError,
    },
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create session: {0}")]
    Scylla(#[from] NewSessionError),
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

        Ok(Client { session })
    }
}

#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
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
            .query_iter(
                "SELECT contents FROM cdb.objects WHERE repository = ? AND hash = ?",
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
        let stmt = self
            .client
            .session
            .prepare("INSERT INTO cdb.objects (repository, hash, contents) VALUES (?, ?, ?)")
            .await?;

        let mut data = vec![];
        object.write_to(&mut data)?;

        self.client
            .session
            .execute_unpaged(&stmt, (self.name.to_string(), id.as_slice(), data))
            .await?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum WriteObjectError {
    #[error("failed to prepare statement: {0}")]
    Prepare(#[from] PrepareError),

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
    pub fn fqid(&self) -> String {
        format!("{}/{}", self.repo.name, self.id)
    }

    pub async fn get(&self) -> Result<Deployment, DeploymentQueryError> {
        self.repo.client.deployment(&self.repo.name, &self.id).await
    }

    pub async fn set(&self, state: DeploymentState) -> Result<(), SetDeploymentError> {
        let deployment = Deployment {
            repository: self.repo.name.clone(),
            id: self.id.clone(),
            created_at: Utc::now(),
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
        let pager = self.session.query_iter(
            "SELECT created_at, state FROM cdb.deployments WHERE repository = ? AND ref_name = ? AND commit_hash = ?",
            (repo.to_string(), &deployment_id.ref_name, deployment_id.commit_hash.as_slice())
        ).await?;

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
    ) -> Result<impl Stream<Item = Result<Deployment, DeploymentQueryError>>, DeploymentQueryError>
    {
        let pager = self
            .session
            .query_iter(
                "SELECT repository, ref_name, commit_hash, created_at, state FROM cdb.active_deployments",
                (),
            )
            .await?;

        Ok(pager
            .rows_stream::<(String, String, Vec<u8>, DateTime<Utc>, String)>()?
            .map(move |r| {
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
            }))
    }
}

#[derive(Error, Debug)]
pub enum DeploymentQueryError {
    #[error("failed to execute: {0}")]
    ScyllaPager(#[from] PagerExecutionError),

    #[error("failed to parse row: {0}")]
    ScyllaTypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    ScyllaNextRow(#[from] NextRowError),

    #[error("{0}")]
    InvalidState(#[from] InvalidDeploymentState),

    #[error("{0}")]
    InvalidRepositoryName(#[from] InvalidRepositoryName),

    #[error("deployment not found")]
    NotFound,
}

impl Client {
    pub async fn set_deployment(&self, deployment: Deployment) -> Result<(), SetDeploymentError> {
        let stmt = self.session.prepare("UPDATE cdb.deployments SET created_at = ?, state = ? WHERE repository = ? AND ref_name = ? AND commit_hash = ?").await?;

        self.session
            .execute_unpaged(
                &stmt,
                (
                    deployment.created_at,
                    deployment.state.to_string(),
                    deployment.repository.to_string(),
                    deployment.id.ref_name,
                    deployment.id.commit_hash.as_slice(),
                ),
            )
            .await?;

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum SetDeploymentError {
    #[error("failed to prepare statement: {0}")]
    Prepare(#[from] PrepareError),

    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),
}
