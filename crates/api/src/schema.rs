use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use juniper::FieldResult;

use crate::json_scalar::JsonValue;
use crate::{Context, field_error, internal_error};

pub(crate) struct AuthSuccess {
    pub(crate) user: SignedInUser,
    pub(crate) token: String,
}

#[juniper::graphql_object(Context = Context)]
impl AuthSuccess {
    fn user(&self) -> &SignedInUser {
        &self.user
    }

    fn token(&self) -> &str {
        &self.token
    }
}

pub(crate) struct User {
    pub(crate) user: udb::User,
}

#[juniper::graphql_object(Context = Context)]
impl User {
    fn username(&self) -> &str {
        &self.user.username
    }

    fn email(&self) -> &str {
        &self.user.email
    }

    fn fullname(&self) -> Option<&str> {
        self.user.fullname.as_deref()
    }
}

pub(crate) struct SignedInUser {
    pub(crate) user: udb::User,
}

#[juniper::graphql_object(Context = Context)]
impl SignedInUser {
    fn username(&self) -> &str {
        &self.user.username
    }

    fn email(&self) -> &str {
        &self.user.email
    }

    fn fullname(&self) -> Option<&str> {
        self.user.fullname.as_deref()
    }

    #[graphql(name = "publicKeys")]
    async fn public_keys(&self, context: &Context) -> FieldResult<Vec<String>> {
        context
            .udb_client
            .user(&self.user.username)
            .pubkeys()
            .list()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list public keys: {}", e);
                internal_error()
            })
    }
}

pub(crate) struct Organization {
    pub(crate) name: ids::OrgId,
}

#[juniper::graphql_object(Context = Context)]
impl Organization {
    fn name(&self) -> String {
        self.name.to_string()
    }

    async fn members(&self, context: &Context) -> FieldResult<Vec<User>> {
        let usernames = context
            .udb_client
            .org(self.name.as_str())
            .members()
            .list()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list org members for {}: {}", self.name, e);
                internal_error()
            })?;

        let mut users = Vec::with_capacity(usernames.len());
        for username in usernames {
            match context.udb_client.user(&username).get().await {
                Ok(user) => users.push(User { user }),
                Err(udb::UserQueryError::NotFound) => {
                    tracing::warn!("Org member {} not found in UDB", username);
                }
                Err(e) => {
                    tracing::error!("Failed to fetch org member {}: {}", username, e);
                    return Err(internal_error());
                }
            }
        }

        Ok(users)
    }

    async fn repository(&self, context: &Context, name: String) -> FieldResult<Repository> {
        let repo: ids::RepoId = name
            .parse()
            .map_err(|_| field_error("Invalid repository name"))?;
        let repo_qid = ids::RepoQid::new(self.name.clone(), repo);
        let repository = context
            .cdb_client
            .repository(&repo_qid)
            .await
            .map_err(|e| {
                tracing::error!("Failed to find repository {repo_qid}: {e}");
                internal_error()
            })?;
        Ok(Repository { repository })
    }

    async fn repositories(&self, context: &Context) -> FieldResult<Vec<Repository>> {
        context
            .cdb_client
            .repositories_by_organization(self.name.to_string())
            .await
            .map_err(|e| {
                tracing::error!("Failed to list repositories for {}: {}", self.name, e);
                internal_error()
            })?
            .map(|repository| repository.map(|repository| Repository { repository }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read repository for {}: {}", self.name, e);
                internal_error()
            })
    }
}

pub(crate) struct Repository {
    pub(crate) repository: cdb::Repository,
}

#[juniper::graphql_object(Context = Context)]
impl Repository {
    fn organization(&self) -> Organization {
        Organization {
            name: self.repository.name.org.clone(),
        }
    }

    fn name(&self) -> String {
        self.repository.name.repo.to_string()
    }

    async fn environment(&self, context: &Context, name: String) -> FieldResult<Environment> {
        let env: ids::EnvironmentId = name
            .parse()
            .map_err(|_| field_error("Invalid environment name"))?;
        let qid = self.repository.name.environment(env);
        let deployments = context
            .cdb_client
            .repo(self.repository.name.clone())
            .deployments()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list deployments for {qid}: {e}");
                internal_error()
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read deployments for {qid}: {e}");
                internal_error()
            })?;

        let deployments: Vec<_> = deployments
            .into_iter()
            .filter(|d| d.environment_qid() == qid)
            .collect();

        Ok(Environment { qid, deployments })
    }

    async fn commit(&self, context: &Context, hash: String) -> FieldResult<Commit> {
        let deployment_id: ids::DeploymentId = hash
            .parse()
            .map_err(|_| field_error("Invalid commit hash"))?;
        let repo_client = context.cdb_client.repo(self.repository.name.clone());
        let oid = gix_hash::ObjectId::from_hex(deployment_id.as_str().as_bytes()).map_err(|e| {
            tracing::error!("Invalid object ID hex for commit {deployment_id}: {e}");
            internal_error()
        })?;
        let commit = repo_client.read_commit(oid).await.map_err(|e| {
            tracing::error!("Failed to read commit {oid}: {e}");
            internal_error()
        })?;
        Ok(Commit {
            repo: self.repository.name.clone(),
            hash: oid,
            commit,
        })
    }

    async fn environments(&self, context: &Context) -> FieldResult<Vec<Environment>> {
        let deployments = context
            .cdb_client
            .repo(self.repository.name.clone())
            .deployments()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to list deployments for {}: {}",
                    self.repository.name,
                    e
                );
                internal_error()
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to read deployments for {}: {}",
                    self.repository.name,
                    e
                );
                internal_error()
            })?;

        let mut env_map: std::collections::BTreeMap<String, Vec<cdb::Deployment>> =
            std::collections::BTreeMap::new();
        for deployment in deployments {
            let env_key = deployment.environment_qid().to_string();
            env_map.entry(env_key).or_default().push(deployment);
        }

        Ok(env_map
            .into_values()
            .map(|deployments| {
                let qid = deployments[0].environment_qid();
                Environment { qid, deployments }
            })
            .collect())
    }
}

pub(crate) struct Environment {
    pub(crate) qid: ids::EnvironmentQid,
    pub(crate) deployments: Vec<cdb::Deployment>,
}

#[juniper::graphql_object(Context = Context)]
impl Environment {
    fn name(&self) -> String {
        self.qid.environment.to_string()
    }

    fn qid(&self) -> String {
        self.qid.to_string()
    }

    async fn deployment(&self, context: &Context, commit: String) -> FieldResult<Deployment> {
        let deployment_id: ids::DeploymentId = commit
            .parse()
            .map_err(|_| field_error("Invalid commit hash"))?;
        let deployment = context
            .cdb_client
            .find_deployment(&self.qid.repo, &self.qid.environment, &deployment_id)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to find deployment {deployment_id} in {}: {e}",
                    self.qid
                );
                internal_error()
            })?;
        Ok(Deployment { deployment })
    }

    fn deployments(&self) -> Vec<Deployment> {
        self.deployments
            .iter()
            .map(|deployment| Deployment {
                deployment: deployment.clone(),
            })
            .collect()
    }

    async fn resource(&self, context: &Context, id: String) -> FieldResult<Option<Resource>> {
        let resource_id: ids::ResourceId =
            id.parse().map_err(|_| field_error("Invalid resource ID"))?;
        let namespace = self.qid.to_string();

        context
            .rdb_client
            .namespace(namespace.clone())
            .resource(
                resource_id.resource_type().to_string(),
                resource_id.resource_name().to_string(),
            )
            .get()
            .await
            .map(|resource| resource.map(|resource| Resource { resource }))
            .map_err(|e| {
                tracing::error!(
                    "Failed to get resource {id} in environment namespace {namespace}: {e}"
                );
                internal_error()
            })
    }

    async fn resources(&self, context: &Context) -> FieldResult<Vec<Resource>> {
        let namespace = self.qid.to_string();

        context
            .rdb_client
            .namespace(namespace.clone())
            .list_resources()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to list resources for environment namespace {namespace}: {e}"
                );
                internal_error()
            })?
            .map(|resource| resource.map(|resource| Resource { resource }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to load resources for environment namespace {namespace}: {e}"
                );
                internal_error()
            })
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let mut all_logs = Vec::new();

        for deployment in &self.deployments {
            let deployment_qid = deployment.deployment_qid().to_string();
            match crate::subscriptions::load_logs(context, deployment_qid.clone(), amount).await {
                Ok(logs) => all_logs.extend(logs),
                Err(error) => {
                    tracing::warn!("Failed to fetch logs for deployment {deployment_qid}: {error}");
                }
            }
        }

        all_logs.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
        all_logs.truncate(amount as usize);
        Ok(all_logs)
    }

    async fn artifacts(&self, context: &Context) -> FieldResult<Vec<Artifact>> {
        let namespace = self.qid.to_string();
        let artifacts = context.adb_client.list(&namespace).await.map_err(|error| {
            tracing::error!("Failed to list artifacts for environment {namespace}: {error}");
            internal_error()
        })?;

        Ok(artifacts
            .into_iter()
            .map(|header| Artifact { header })
            .collect())
    }
}

pub(crate) struct Commit {
    pub(crate) repo: ids::RepoQid,
    pub(crate) hash: gix_hash::ObjectId,
    pub(crate) commit: gix_object::Commit,
}

#[juniper::graphql_object(Context = Context)]
impl Commit {
    fn hash(&self) -> String {
        self.hash.to_string()
    }

    fn message(&self) -> String {
        String::from_utf8_lossy(&self.commit.message).into_owned()
    }

    async fn tree(&self, context: &Context) -> FieldResult<Tree> {
        let repo_client = context.cdb_client.repo(self.repo.clone());
        let tree = repo_client.read_tree(self.commit.tree).await.map_err(|e| {
            tracing::error!("Failed to read tree {}: {e}", self.commit.tree);
            internal_error()
        })?;
        Ok(Tree {
            repo: self.repo.clone(),
            hash: self.commit.tree,
            name: None,
            tree,
        })
    }

    #[graphql(name = "treeEntry")]
    async fn tree_entry(&self, context: &Context, path: String) -> FieldResult<Option<TreeEntry>> {
        let repo_client = context.cdb_client.repo(self.repo.clone());
        let root_tree = repo_client.read_tree(self.commit.tree).await.map_err(|e| {
            tracing::error!("Failed to read root tree {}: {e}", self.commit.tree);
            internal_error()
        })?;

        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return Ok(Some(TreeEntry::Tree(Tree {
                repo: self.repo.clone(),
                hash: self.commit.tree,
                name: None,
                tree: root_tree,
            })));
        }

        let mut current_tree = root_tree;
        for (i, segment) in segments.iter().enumerate() {
            let entry = current_tree
                .entries
                .iter()
                .find(|e| e.filename.as_slice() == segment.as_bytes());

            let Some(entry) = entry else {
                return Ok(None);
            };

            if i == segments.len() - 1 {
                // Last segment: return the entry
                let name = Some(String::from_utf8_lossy(entry.filename.as_slice()).into_owned());
                if entry.mode.is_tree() {
                    let tree = repo_client.read_tree(entry.oid).await.map_err(|e| {
                        tracing::error!("Failed to read tree {}: {e}", entry.oid);
                        internal_error()
                    })?;
                    return Ok(Some(TreeEntry::Tree(Tree {
                        repo: self.repo.clone(),
                        hash: entry.oid,
                        name,
                        tree,
                    })));
                } else if entry.mode.is_blob() {
                    let blob = repo_client.read_blob(entry.oid).await.map_err(|e| {
                        tracing::error!("Failed to read blob {}: {e}", entry.oid);
                        internal_error()
                    })?;
                    return Ok(Some(TreeEntry::Blob(Blob {
                        hash: entry.oid,
                        name,
                        blob,
                    })));
                } else {
                    return Ok(None);
                }
            }

            // Intermediate segment: must be a tree
            if !entry.mode.is_tree() {
                return Ok(None);
            }
            current_tree = repo_client.read_tree(entry.oid).await.map_err(|e| {
                tracing::error!("Failed to read tree {}: {e}", entry.oid);
                internal_error()
            })?;
        }

        Ok(None)
    }
}

pub(crate) struct Tree {
    repo: ids::RepoQid,
    hash: gix_hash::ObjectId,
    name: Option<String>,
    tree: gix_object::Tree,
}

#[juniper::graphql_object(Context = Context)]
impl Tree {
    fn hash(&self) -> String {
        self.hash.to_string()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    async fn entries(&self, context: &Context) -> FieldResult<Vec<TreeEntry>> {
        let repo_client = context.cdb_client.repo(self.repo.clone());
        let mut entries = Vec::with_capacity(self.tree.entries.len());

        for entry in &self.tree.entries {
            let name = Some(String::from_utf8_lossy(entry.filename.as_slice()).into_owned());
            if entry.mode.is_tree() {
                let tree = repo_client.read_tree(entry.oid).await.map_err(|e| {
                    tracing::error!("Failed to read tree entry {}: {e}", entry.oid);
                    internal_error()
                })?;
                entries.push(TreeEntry::Tree(Tree {
                    repo: self.repo.clone(),
                    hash: entry.oid,
                    name,
                    tree,
                }));
            } else if entry.mode.is_blob() {
                let blob = repo_client.read_blob(entry.oid).await.map_err(|e| {
                    tracing::error!("Failed to read blob entry {}: {e}", entry.oid);
                    internal_error()
                })?;
                entries.push(TreeEntry::Blob(Blob {
                    hash: entry.oid,
                    name,
                    blob,
                }));
            }
            // Skip non-tree/non-blob entries (e.g., submodule commits)
        }

        Ok(entries)
    }
}

pub(crate) struct Blob {
    hash: gix_hash::ObjectId,
    name: Option<String>,
    blob: gix_object::Blob,
}

#[juniper::graphql_object(Context = Context)]
impl Blob {
    fn hash(&self) -> String {
        self.hash.to_string()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn size(&self) -> i32 {
        self.blob.data.len() as i32
    }

    fn content(&self) -> Option<String> {
        std::str::from_utf8(&self.blob.data).ok().map(String::from)
    }
}

#[derive(juniper::GraphQLUnion)]
#[graphql(Context = Context)]
pub(crate) enum TreeEntry {
    Blob(Blob),
    Tree(Tree),
}

pub(crate) struct Deployment {
    pub(crate) deployment: cdb::Deployment,
}

#[juniper::graphql_object(Context = Context)]
impl Deployment {
    fn id(&self) -> String {
        self.deployment.deployment_qid().to_string()
    }

    #[graphql(name = "ref")]
    fn r#ref(&self) -> String {
        self.deployment.environment.to_string()
    }

    async fn commit(&self, context: &Context) -> FieldResult<Commit> {
        let repo_client = context.cdb_client.repo(self.deployment.repo.clone());
        let hash = gix_hash::ObjectId::from_hex(self.deployment.deployment.as_str().as_bytes())
            .map_err(|e| {
                tracing::error!(
                    "Invalid object ID hex for deployment {}: {e}",
                    self.deployment.deployment
                );
                internal_error()
            })?;
        let commit = repo_client.read_commit(hash).await.map_err(|e| {
            tracing::error!("Failed to read commit {hash}: {e}");
            internal_error()
        })?;
        Ok(Commit {
            repo: self.deployment.repo.clone(),
            hash,
            commit,
        })
    }

    #[graphql(name = "createdAt")]
    fn created_at(&self) -> String {
        self.deployment.created_at.to_rfc3339()
    }

    fn state(&self) -> DeploymentState {
        self.deployment.state.into()
    }

    async fn resources(&self, context: &Context) -> FieldResult<Vec<Resource>> {
        let namespace = self.deployment.environment_qid().to_string();
        let owner = self.deployment.deployment_qid().to_string();

        context
            .rdb_client
            .namespace(namespace.clone())
            .list_resources_by_owner(&owner)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to list resources for deployment namespace {namespace} and owner {owner}: {e}"
                );
                internal_error()
            })?
            .map(|resource| resource.map(|resource| Resource { resource }))
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to load resources for deployment namespace {namespace} and owner {owner}: {e}"
                );
                internal_error()
            })
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let deployment_qid = self.deployment.deployment_qid().to_string();
        crate::subscriptions::load_logs(context, deployment_qid.clone(), amount)
            .await
            .map_err(|error| {
                tracing::error!("Failed to fetch deployment logs for {deployment_qid}: {error}");
                internal_error()
            })
    }
}

pub(crate) struct Artifact {
    header: adb::ArtifactHeader,
}

#[juniper::graphql_object(Context = Context)]
impl Artifact {
    fn name(&self) -> &str {
        self.header.name()
    }

    #[graphql(name = "mediaType")]
    fn media_type(&self) -> &str {
        self.header.media_type()
    }

    async fn url(&self, context: &Context) -> FieldResult<String> {
        context
            .adb_client
            .presign_read_url(
                self.header.namespace(),
                self.header.name(),
                Duration::from_secs(900),
            )
            .await
            .map_err(|error| {
                tracing::error!(
                    "Failed to presign artifact URL for {}/{}: {}",
                    self.header.namespace(),
                    self.header.name(),
                    error
                );
                internal_error()
            })
    }
}

#[derive(juniper::GraphQLObject)]
#[graphql(Context = Context)]
pub(crate) struct SourceFrame {
    #[graphql(name = "moduleId")]
    module_id: String,
    span: String,
    name: String,
}

pub(crate) struct Resource {
    resource: rdb::Resource,
}

impl Resource {
    fn resource_qid(&self) -> FieldResult<ids::ResourceQid> {
        let env_qid: ids::EnvironmentQid = self.resource.namespace.parse().map_err(|_| {
            tracing::error!(
                "Invalid environment QID in resource namespace: {}",
                self.resource.namespace
            );
            internal_error()
        })?;
        let resource_id = ids::ResourceId::new(&self.resource.resource_type, &self.resource.name);
        Ok(ids::ResourceQid::new(env_qid, resource_id))
    }
}

#[juniper::graphql_object(Context = Context)]
impl Resource {
    #[graphql(name = "type")]
    fn r#type(&self) -> &str {
        &self.resource.resource_type
    }

    fn name(&self) -> &str {
        &self.resource.name
    }

    fn inputs(&self) -> FieldResult<Option<JsonValue>> {
        self.resource
            .inputs
            .as_ref()
            .map(|record| {
                serde_json::to_value(record)
                    .map(JsonValue)
                    .map_err(|error| {
                        tracing::error!("Failed to serialize resource inputs to JSON: {error}");
                        internal_error()
                    })
            })
            .transpose()
    }

    fn outputs(&self) -> FieldResult<Option<JsonValue>> {
        self.resource
            .outputs
            .as_ref()
            .map(|record| {
                serde_json::to_value(record)
                    .map(JsonValue)
                    .map_err(|error| {
                        tracing::error!("Failed to serialize resource outputs to JSON: {error}");
                        internal_error()
                    })
            })
            .transpose()
    }

    async fn owner(&self, context: &Context) -> FieldResult<Option<Deployment>> {
        let Some(owner) = self.resource.owner.as_deref() else {
            return Ok(None);
        };

        let deployment_qid: ids::DeploymentQid = match owner.parse() {
            Ok(qid) => qid,
            Err(_) => {
                tracing::warn!("invalid resource owner deployment QID format: {owner}");
                return Ok(None);
            }
        };

        let repo_qid = deployment_qid.repo_qid().clone();

        let deployments = context
            .cdb_client
            .repo(repo_qid.clone())
            .deployments()
            .await
            .map_err(|e| {
                tracing::error!("Failed to list deployments for owner repository {repo_qid}: {e}");
                internal_error()
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| {
                tracing::error!("Failed to read deployments for owner repository {repo_qid}: {e}");
                internal_error()
            })?;

        Ok(deployments
            .into_iter()
            .find(|deployment| deployment.deployment_qid().to_string() == owner)
            .map(|deployment| Deployment { deployment }))
    }

    async fn dependencies(&self, context: &Context) -> FieldResult<Vec<Resource>> {
        let mut dependencies = Vec::with_capacity(self.resource.dependencies.len());

        for dependency in &self.resource.dependencies {
            let resource = context
                .rdb_client
                .namespace(self.resource.namespace.clone())
                .resource(dependency.typ.clone(), dependency.name.clone())
                .get()
                .await
                .map_err(|error| {
                    tracing::error!(
                        "Failed to load dependency {dependency} in namespace {}: {error}",
                        self.resource.namespace,
                    );
                    internal_error()
                })?;

            if let Some(resource) = resource {
                dependencies.push(Resource { resource });
            }
        }

        Ok(dependencies)
    }

    fn markers(&self) -> Vec<ResourceMarker> {
        self.resource
            .markers
            .iter()
            .copied()
            .map(ResourceMarker::from)
            .collect()
    }

    #[graphql(name = "sourceTrace")]
    fn source_trace(&self) -> Vec<SourceFrame> {
        self.resource
            .source_trace
            .iter()
            .map(|f| SourceFrame {
                module_id: f.module_id.clone(),
                span: f.span.clone(),
                name: f.name.clone(),
            })
            .collect()
    }

    #[graphql(name = "lastLogs")]
    async fn last_logs(&self, context: &Context, amount: Option<i32>) -> FieldResult<Vec<Log>> {
        let amount = amount.unwrap_or(20).max(0) as u64;
        let resource_qid = self.resource_qid()?.to_string();
        crate::subscriptions::load_logs(context, resource_qid.clone(), amount)
            .await
            .map_err(|error| {
                tracing::error!("Failed to fetch resource logs for {resource_qid}: {error}");
                internal_error()
            })
    }
}

#[derive(Clone, Copy, juniper::GraphQLEnum)]
pub(crate) enum DeploymentState {
    #[graphql(name = "DOWN")]
    Down,
    #[graphql(name = "UNDESIRED")]
    Undesired,
    #[graphql(name = "LINGERING")]
    Lingering,
    #[graphql(name = "DESIRED")]
    Desired,
    #[graphql(name = "UP")]
    Up,
    #[graphql(name = "FAILING")]
    Failing,
    #[graphql(name = "FAILED")]
    Failed,
}

impl From<cdb::DeploymentState> for DeploymentState {
    fn from(state: cdb::DeploymentState) -> Self {
        match state {
            cdb::DeploymentState::Down => DeploymentState::Down,
            cdb::DeploymentState::Undesired => DeploymentState::Undesired,
            cdb::DeploymentState::Lingering => DeploymentState::Lingering,
            cdb::DeploymentState::Desired => DeploymentState::Desired,
            cdb::DeploymentState::Up => DeploymentState::Up,
            cdb::DeploymentState::Failing => DeploymentState::Failing,
            cdb::DeploymentState::Failed => DeploymentState::Failed,
        }
    }
}

#[derive(Clone, Copy, juniper::GraphQLEnum)]
pub(crate) enum ResourceMarker {
    #[graphql(name = "VOLATILE")]
    Volatile,
    #[graphql(name = "STICKY")]
    Sticky,
}

impl From<sclc::Marker> for ResourceMarker {
    fn from(marker: sclc::Marker) -> Self {
        match marker {
            sclc::Marker::Volatile => ResourceMarker::Volatile,
            sclc::Marker::Sticky => ResourceMarker::Sticky,
        }
    }
}

#[derive(Clone, Copy, Debug, juniper::GraphQLEnum)]
pub(crate) enum Severity {
    #[graphql(name = "INFO")]
    Info,
    #[graphql(name = "WARNING")]
    Warning,
    #[graphql(name = "ERROR")]
    Err,
}

impl From<ldb::Severity> for Severity {
    fn from(severity: ldb::Severity) -> Self {
        match severity {
            ldb::Severity::Info => Severity::Info,
            ldb::Severity::Warning => Severity::Warning,
            ldb::Severity::Error => Severity::Err,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Log {
    pub(crate) severity: Severity,
    pub(crate) timestamp: String,
    pub(crate) message: String,
}

#[juniper::graphql_object(Context = Context)]
impl Log {
    fn severity(&self) -> Severity {
        self.severity
    }

    fn timestamp(&self) -> &str {
        &self.timestamp
    }

    fn message(&self) -> &str {
        &self.message
    }
}

pub(crate) struct AuthChallenge {
    pub(crate) challenge: String,
    pub(crate) taken: bool,
    pub(crate) passkey_registration: JsonValue,
    pub(crate) passkey_signin: Option<JsonValue>,
}

#[juniper::graphql_object(Context = Context)]
impl AuthChallenge {
    fn challenge(&self) -> &str {
        &self.challenge
    }

    fn taken(&self) -> bool {
        self.taken
    }

    #[graphql(name = "passkeyRegistration")]
    fn passkey_registration(&self) -> &JsonValue {
        &self.passkey_registration
    }

    #[graphql(name = "passkeySignin")]
    fn passkey_signin(&self) -> Option<&JsonValue> {
        self.passkey_signin.as_ref()
    }
}
