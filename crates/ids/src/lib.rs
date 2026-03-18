//! # Skyr Identifier Types
//!
//! This crate defines the standard vocabulary and types for identifying resources
//! across all layers of the Skyr architecture (databases, protocols, APIs, runtime).
//!
//! ## Hierarchy
//!
//! Skyr uses a four-level namespace hierarchy:
//!
//! 1. **Organization** — the top-level namespace for resources accessible by multiple users.
//!    Currently, the only type of organization is the user's own username.
//!
//! 2. **Repository** — the name of the codebase for a project (e.g. a Git repository).
//!
//! 3. **Environment** — one instance of the DAG of resources for a repository. Environments
//!    are identified by Git refs (branch or tag names). Only one deployment can be "desired"
//!    per environment, and the resource lifecycle and supersession mechanic is grouped under
//!    the environment.
//!
//! 4. **Deployment** — a revision of an environment. Deployments own resources temporarily
//!    (since adoption can change the owner), but all resources belong to the same environment
//!    during their entire lifecycle. Deployments are identified by commit hash.
//!
//! ## IDs vs QIDs
//!
//! Each level has an **ID** (the unqualified, local segment) and a **QID** (the fully
//! qualified identifier that includes all parent scopes):
//!
//! | Level | ID Example | QID Format | QID Example |
//! |-------|-----------|------------|-------------|
//! | Org | `MyOrg` | N/A (top level) | `MyOrg` |
//! | Repo | `MyRepo` | `OrgId/RepoId` | `MyOrg/MyRepo` |
//! | Env | `main` | `RepoQid::EnvironmentId` | `MyOrg/MyRepo::main` |
//! | Deploy | `2cbec...` | `EnvironmentQid@DeploymentId` | `MyOrg/MyRepo::main@2cbec...` |
//! | Resource | `Std/Random.Int:seed` | `EnvironmentQid::ResourceId` | `MyOrg/MyRepo::main::Std/Random.Int:seed` |
//!
//! ## Separators
//!
//! - `/` between organization and repository
//! - `::` between repository and environment
//! - `@` between environment and deployment
//! - `:` between resource type and resource name (within a resource ID)
//! - `::` between environment QID and resource ID (within a resource QID)
//!
//! ## Namespaces
//!
//! Some infrastructure doesn't care which level of the hierarchy is being used as its
//! partition key. For these cases, use the term **"namespace"** with plain `String`/`&str`
//! values — the caller decides which QID level to use. For example, the LDB (log database)
//! accepts any QID string as its namespace, so you can have "org-level logs" or
//! "deployment-level logs" depending on what QID you pass.
//!
//! ## Environment ID Conventions
//!
//! Environment IDs are derived from Git refs with the `refs/heads/` or `refs/tags/` prefix
//! stripped. To disambiguate between branches and tags:
//!
//! - **Branches** use bare names: `main`, `feature/my-branch`
//! - **Tags** use a `tag:` prefix: `tag:v1.0`, `tag:release/2024`
//!
//! Use [`EnvironmentId::from_git_ref`] and [`EnvironmentId::to_git_ref`] to convert
//! between environment IDs and full Git ref paths.
//!
//! ## Extending QIDs
//!
//! Other parts of the architecture can freely define their own QIDs by picking an unambiguous
//! separator and appending an ID segment to an existing QID. For example, pod and container
//! names can be prefixed with an environment QID:
//!
//! - Pod QID: `MyOrg/MyRepo::main::my-pod`
//! - Container QID: `MyOrg/MyRepo::main::my-pod/my-container`

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Returns `true` if `s` is a valid SCL symbol: non-empty, first character is
/// alphabetic or underscore, remaining characters are alphanumeric or underscore.
fn is_valid_symbol(s: &str) -> bool {
    let [first, rest @ ..] = s.as_bytes() else {
        return false;
    };
    (first.is_ascii_alphabetic() || *first == b'_')
        && rest.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Returns `true` if `s` is a valid 40-character lowercase hexadecimal string.
fn is_valid_oid_hex(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Error returned when parsing an invalid identifier.
#[derive(Error, Debug, Clone)]
pub enum ParseIdError {
    #[error("invalid organization ID: {0:?} (must be a valid SCL symbol)")]
    InvalidOrgId(String),

    #[error("invalid repository ID: {0:?} (must be a valid SCL symbol)")]
    InvalidRepoId(String),

    #[error("invalid repository QID: {0:?} (expected format: OrgId/RepoId)")]
    InvalidRepoQid(String),

    #[error("invalid environment ID: {0:?} (must not be empty)")]
    InvalidEnvironmentId(String),

    #[error("invalid environment QID: {0:?} (expected format: OrgId/RepoId::EnvironmentId)")]
    InvalidEnvironmentQid(String),

    #[error("invalid deployment ID: {0:?} (must be 40-char lowercase hex)")]
    InvalidDeploymentId(String),

    #[error(
        "invalid deployment QID: {0:?} (expected format: OrgId/RepoId::EnvironmentId@DeploymentId)"
    )]
    InvalidDeploymentQid(String),

    #[error("invalid resource ID: {0:?} (expected format: ResourceType:ResourceName)")]
    InvalidResourceId(String),

    #[error(
        "invalid resource QID: {0:?} (expected format: OrgId/RepoId::EnvironmentId::ResourceType:ResourceName)"
    )]
    InvalidResourceQid(String),

    #[error("invalid git ref: {0:?} (expected refs/heads/... or refs/tags/...)")]
    InvalidGitRef(String),
}

// ---------------------------------------------------------------------------
// OrgId
// ---------------------------------------------------------------------------

/// Organization ID. The top-level namespace for resources.
///
/// Validated as an SCL symbol: non-empty, starts with an alphabetic character
/// or underscore, followed by alphanumeric characters or underscores.
///
/// Currently, the only type of organization is the user's own username.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OrgId(String);

impl OrgId {
    /// Creates a new `OrgId` without validation. Use `FromStr` for validated construction.
    pub fn new_unchecked(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the organization ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OrgId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for OrgId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OrgId({self})")
    }
}

impl FromStr for OrgId {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !is_valid_symbol(s) {
            return Err(ParseIdError::InvalidOrgId(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }
}

impl From<OrgId> for String {
    fn from(id: OrgId) -> Self {
        id.0
    }
}

impl TryFrom<String> for OrgId {
    type Error = ParseIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if !is_valid_symbol(&s) {
            return Err(ParseIdError::InvalidOrgId(s));
        }
        Ok(Self(s))
    }
}

// ---------------------------------------------------------------------------
// RepoId
// ---------------------------------------------------------------------------

/// Repository ID. The name of a codebase within an organization.
///
/// Validated as an SCL symbol, same rules as [`OrgId`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoId(String);

impl RepoId {
    /// Creates a new `RepoId` without validation.
    pub fn new_unchecked(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the repository ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RepoId({self})")
    }
}

impl FromStr for RepoId {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !is_valid_symbol(s) {
            return Err(ParseIdError::InvalidRepoId(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }
}

impl From<RepoId> for String {
    fn from(id: RepoId) -> Self {
        id.0
    }
}

impl TryFrom<String> for RepoId {
    type Error = ParseIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if !is_valid_symbol(&s) {
            return Err(ParseIdError::InvalidRepoId(s));
        }
        Ok(Self(s))
    }
}

// ---------------------------------------------------------------------------
// RepoQid
// ---------------------------------------------------------------------------

/// Qualified repository identifier: `OrgId/RepoId`.
///
/// This uniquely identifies a repository within the Skyr system.
///
/// # Examples
///
/// ```
/// use ids::RepoQid;
/// let qid: RepoQid = "MyOrg/MyRepo".parse().unwrap();
/// assert_eq!(qid.to_string(), "MyOrg/MyRepo");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoQid {
    pub org: OrgId,
    pub repo: RepoId,
}

impl RepoQid {
    /// Creates a new `RepoQid` from an org and repo ID.
    pub fn new(org: OrgId, repo: RepoId) -> Self {
        Self { org, repo }
    }

    /// Creates an [`EnvironmentQid`] by combining this repo QID with an environment ID.
    pub fn environment(&self, environment: EnvironmentId) -> EnvironmentQid {
        EnvironmentQid {
            repo: self.clone(),
            environment,
        }
    }
}

impl fmt::Display for RepoQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.repo)
    }
}

impl fmt::Debug for RepoQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RepoQid({self})")
    }
}

impl FromStr for RepoQid {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((org, repo)) = s.split_once('/') else {
            return Err(ParseIdError::InvalidRepoQid(s.to_string()));
        };
        Ok(Self {
            org: org
                .parse()
                .map_err(|_| ParseIdError::InvalidRepoQid(s.to_string()))?,
            repo: repo
                .parse()
                .map_err(|_| ParseIdError::InvalidRepoQid(s.to_string()))?,
        })
    }
}

impl Serialize for RepoQid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RepoQid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// EnvironmentId
// ---------------------------------------------------------------------------

/// Environment ID. Identifies a single instance of the resource DAG for a repository.
///
/// Environments are identified by Git refs. The environment ID is the ref name with the
/// `refs/heads/` or `refs/tags/` prefix stripped:
///
/// - **Branches** use bare names: `main`, `feature/my-branch`
/// - **Tags** use a `tag:` prefix: `tag:v1.0`, `tag:release/2024`
///
/// Use [`EnvironmentId::from_git_ref`] to convert from a full Git ref path, and
/// [`EnvironmentId::to_git_ref`] to reconstruct the full ref path.
///
/// # Examples
///
/// ```
/// use ids::EnvironmentId;
///
/// let env = EnvironmentId::from_git_ref("refs/heads/main").unwrap();
/// assert_eq!(env.as_str(), "main");
/// assert_eq!(env.to_git_ref(), "refs/heads/main");
/// assert!(!env.is_tag());
///
/// let env = EnvironmentId::from_git_ref("refs/tags/v1.0").unwrap();
/// assert_eq!(env.as_str(), "tag:v1.0");
/// assert_eq!(env.to_git_ref(), "refs/tags/v1.0");
/// assert!(env.is_tag());
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EnvironmentId(String);

impl EnvironmentId {
    /// Creates a new `EnvironmentId` without validation.
    pub fn new_unchecked(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the environment ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if this environment represents a Git tag (has the `tag:` prefix).
    pub fn is_tag(&self) -> bool {
        self.0.starts_with("tag:")
    }

    /// Creates an `EnvironmentId` from a full Git ref path like `refs/heads/main`
    /// or `refs/tags/v1.0`.
    ///
    /// - `refs/heads/main` → `EnvironmentId("main")`
    /// - `refs/tags/v1.0` → `EnvironmentId("tag:v1.0")`
    pub fn from_git_ref(full_ref: &str) -> Result<Self, ParseIdError> {
        if let Some(name) = full_ref.strip_prefix("refs/heads/") {
            if name.is_empty() {
                return Err(ParseIdError::InvalidGitRef(full_ref.to_string()));
            }
            Ok(Self(name.to_string()))
        } else if let Some(name) = full_ref.strip_prefix("refs/tags/") {
            if name.is_empty() {
                return Err(ParseIdError::InvalidGitRef(full_ref.to_string()));
            }
            Ok(Self(format!("tag:{name}")))
        } else {
            Err(ParseIdError::InvalidGitRef(full_ref.to_string()))
        }
    }

    /// Reconstructs the full Git ref path from this environment ID.
    ///
    /// - `EnvironmentId("main")` → `"refs/heads/main"`
    /// - `EnvironmentId("tag:v1.0")` → `"refs/tags/v1.0"`
    pub fn to_git_ref(&self) -> String {
        if let Some(tag_name) = self.0.strip_prefix("tag:") {
            format!("refs/tags/{tag_name}")
        } else {
            format!("refs/heads/{}", self.0)
        }
    }
}

impl fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for EnvironmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvironmentId({self})")
    }
}

impl FromStr for EnvironmentId {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseIdError::InvalidEnvironmentId(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }
}

impl From<EnvironmentId> for String {
    fn from(id: EnvironmentId) -> Self {
        id.0
    }
}

impl TryFrom<String> for EnvironmentId {
    type Error = ParseIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.is_empty() {
            return Err(ParseIdError::InvalidEnvironmentId(s));
        }
        Ok(Self(s))
    }
}

// ---------------------------------------------------------------------------
// EnvironmentQid
// ---------------------------------------------------------------------------

/// Qualified environment identifier: `OrgId/RepoId::EnvironmentId`.
///
/// This uniquely identifies an environment (a single instance of the resource DAG)
/// within the Skyr system.
///
/// # Examples
///
/// ```
/// use ids::EnvironmentQid;
/// let qid: EnvironmentQid = "MyOrg/MyRepo::feature/my-branch".parse().unwrap();
/// assert_eq!(qid.to_string(), "MyOrg/MyRepo::feature/my-branch");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvironmentQid {
    pub repo: RepoQid,
    pub environment: EnvironmentId,
}

impl EnvironmentQid {
    /// Creates a new `EnvironmentQid`.
    pub fn new(repo: RepoQid, environment: EnvironmentId) -> Self {
        Self { repo, environment }
    }

    /// Returns a reference to the repository QID.
    pub fn repo_qid(&self) -> &RepoQid {
        &self.repo
    }

    /// Creates a [`ResourceQid`] by combining this environment QID with a resource ID.
    pub fn resource(&self, resource: ResourceId) -> ResourceQid {
        ResourceQid {
            environment: self.clone(),
            resource,
        }
    }

    /// Creates a [`DeploymentQid`] by combining this environment QID with a deployment ID.
    pub fn deployment(&self, deployment: DeploymentId) -> DeploymentQid {
        DeploymentQid {
            environment: self.clone(),
            deployment,
        }
    }
}

impl fmt::Display for EnvironmentQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.repo, self.environment)
    }
}

impl fmt::Debug for EnvironmentQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EnvironmentQid({self})")
    }
}

impl FromStr for EnvironmentQid {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split on first "::" — the environment ID may contain "/" (e.g., "feature/branch")
        // but not "::".
        let Some((repo_part, env_part)) = s.split_once("::") else {
            return Err(ParseIdError::InvalidEnvironmentQid(s.to_string()));
        };
        Ok(Self {
            repo: repo_part
                .parse()
                .map_err(|_| ParseIdError::InvalidEnvironmentQid(s.to_string()))?,
            environment: env_part
                .parse()
                .map_err(|_| ParseIdError::InvalidEnvironmentQid(s.to_string()))?,
        })
    }
}

impl Serialize for EnvironmentQid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EnvironmentQid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// DeploymentId
// ---------------------------------------------------------------------------

/// Deployment ID. A revision of an environment, identified by a Git commit hash.
///
/// This is a 40-character lowercase hexadecimal string (a Git OID / SHA-1 hash).
///
/// # Examples
///
/// ```
/// use ids::DeploymentId;
/// let id: DeploymentId = "2cbecbed4bfa1599ef4ce0dfc542c97a82d79268".parse().unwrap();
/// assert_eq!(id.as_str().len(), 40);
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeploymentId(String);

impl DeploymentId {
    /// Creates a new `DeploymentId` without validation.
    pub fn new_unchecked(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Returns the deployment ID as a string slice (40-char hex).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a `DeploymentId` from raw bytes (20-byte SHA-1 hash), encoding as hex.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ParseIdError> {
        if bytes.len() != 20 {
            return Err(ParseIdError::InvalidDeploymentId(format!(
                "<{} bytes>",
                bytes.len()
            )));
        }
        use std::fmt::Write;
        let mut hex = String::with_capacity(40);
        for b in bytes {
            write!(hex, "{b:02x}").unwrap();
        }
        Ok(Self(hex))
    }

    /// Decodes the hex string into a 20-byte array.
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        for (i, chunk) in self.0.as_bytes().chunks(2).enumerate() {
            let hi = hex_digit(chunk[0]);
            let lo = hex_digit(chunk[1]);
            out[i] = (hi << 4) | lo;
        }
        out
    }
}

fn hex_digit(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => 0, // Should not happen with validated input
    }
}

impl fmt::Display for DeploymentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for DeploymentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeploymentId({self})")
    }
}

impl FromStr for DeploymentId {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if !is_valid_oid_hex(s) {
            return Err(ParseIdError::InvalidDeploymentId(s.to_string()));
        }
        Ok(Self(s.to_string()))
    }
}

impl From<DeploymentId> for String {
    fn from(id: DeploymentId) -> Self {
        id.0
    }
}

impl TryFrom<String> for DeploymentId {
    type Error = ParseIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if !is_valid_oid_hex(&s) {
            return Err(ParseIdError::InvalidDeploymentId(s));
        }
        Ok(Self(s))
    }
}

// ---------------------------------------------------------------------------
// ResourceId
// ---------------------------------------------------------------------------

/// Resource ID. Identifies a resource within an environment by combining
/// its resource type and resource name, separated by `:`.
///
/// The resource type is the plugin-qualified type (e.g., `Std/Random.Int`)
/// and the resource name is the unique name within that type (e.g., `seed`).
///
/// # Format
///
/// `ResourceType:ResourceName` — for example, `Std/Random.Int:seed`.
///
/// # Examples
///
/// ```
/// use ids::ResourceId;
/// let id: ResourceId = "Std/Random.Int:seed".parse().unwrap();
/// assert_eq!(id.resource_type(), "Std/Random.Int");
/// assert_eq!(id.resource_name(), "seed");
/// assert_eq!(id.to_string(), "Std/Random.Int:seed");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId {
    pub typ: String,
    pub name: String,
}

impl ResourceId {
    /// Creates a new `ResourceId` from a resource type and name.
    pub fn new(typ: impl Into<String>, name: impl Into<String>) -> Self {
        Self { typ: typ.into(), name: name.into() }
    }

    /// Returns the resource type.
    pub fn resource_type(&self) -> &str {
        &self.typ
    }

    /// Returns the resource name.
    pub fn resource_name(&self) -> &str {
        &self.name
    }

    /// Returns the full `Type:Name` string.
    pub fn as_str(&self) -> String {
        format!("{}:{}", self.typ, self.name)
    }
}

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.typ, self.name)
    }
}

impl fmt::Debug for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceId({self})")
    }
}

impl FromStr for ResourceId {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some(sep) = s.find(':') else {
            return Err(ParseIdError::InvalidResourceId(s.to_string()));
        };
        if sep == 0 || sep == s.len() - 1 {
            return Err(ParseIdError::InvalidResourceId(s.to_string()));
        }
        Ok(Self {
            typ: s[..sep].to_string(),
            name: s[sep + 1..].to_string(),
        })
    }
}

impl From<ResourceId> for String {
    fn from(id: ResourceId) -> Self {
        id.to_string()
    }
}

impl TryFrom<String> for ResourceId {
    type Error = ParseIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl Serialize for ResourceId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ResourceId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// ResourceQid
// ---------------------------------------------------------------------------

/// Qualified resource identifier: `EnvironmentQid::ResourceId`.
///
/// This uniquely identifies a resource across the entire Skyr system by
/// combining the environment's qualified identifier with the resource ID.
///
/// # Format
///
/// `OrgId/RepoId::EnvironmentId::ResourceType:ResourceName`
///
/// # Examples
///
/// ```
/// use ids::ResourceQid;
/// let qid: ResourceQid = "MyOrg/MyRepo::main::Std/Random.Int:seed".parse().unwrap();
/// assert_eq!(qid.environment_qid().to_string(), "MyOrg/MyRepo::main");
/// assert_eq!(qid.resource().to_string(), "Std/Random.Int:seed");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceQid {
    pub environment: EnvironmentQid,
    pub resource: ResourceId,
}

impl ResourceQid {
    /// Creates a new `ResourceQid`.
    pub fn new(environment: EnvironmentQid, resource: ResourceId) -> Self {
        Self {
            environment,
            resource,
        }
    }

    /// Returns a reference to the environment QID.
    pub fn environment_qid(&self) -> &EnvironmentQid {
        &self.environment
    }

    /// Returns a reference to the resource ID.
    pub fn resource(&self) -> &ResourceId {
        &self.resource
    }
}

impl fmt::Display for ResourceQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.environment, self.resource)
    }
}

impl fmt::Debug for ResourceQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceQid({self})")
    }
}

impl FromStr for ResourceQid {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // The format is `OrgId/RepoId::EnvironmentId::ResourceType:ResourceName`.
        // We split on the *last* `::` to separate the environment QID from
        // the resource ID, since the environment QID itself contains `::` but
        // the resource ID uses `:` (single colon) as its internal separator.
        let Some((env_part, resource_part)) = s.rsplit_once("::") else {
            return Err(ParseIdError::InvalidResourceQid(s.to_string()));
        };
        Ok(Self {
            environment: env_part
                .parse()
                .map_err(|_| ParseIdError::InvalidResourceQid(s.to_string()))?,
            resource: resource_part
                .parse()
                .map_err(|_| ParseIdError::InvalidResourceQid(s.to_string()))?,
        })
    }
}

impl Serialize for ResourceQid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ResourceQid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// DeploymentQid
// ---------------------------------------------------------------------------

/// Qualified deployment identifier: `OrgId/RepoId::EnvironmentId@DeploymentId`.
///
/// This is the most specific identifier in the hierarchy, uniquely identifying
/// a single deployment (revision) of an environment.
///
/// # Examples
///
/// ```
/// use ids::DeploymentQid;
/// let qid: DeploymentQid = "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268".parse().unwrap();
/// assert_eq!(qid.to_string(), "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268");
/// assert_eq!(qid.environment_qid().to_string(), "MyOrg/MyRepo::main");
/// assert_eq!(qid.repo_qid().to_string(), "MyOrg/MyRepo");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeploymentQid {
    pub environment: EnvironmentQid,
    pub deployment: DeploymentId,
}

impl DeploymentQid {
    /// Creates a new `DeploymentQid`.
    pub fn new(environment: EnvironmentQid, deployment: DeploymentId) -> Self {
        Self {
            environment,
            deployment,
        }
    }

    /// Returns a reference to the environment QID.
    pub fn environment_qid(&self) -> &EnvironmentQid {
        &self.environment
    }

    /// Returns a reference to the repository QID.
    pub fn repo_qid(&self) -> &RepoQid {
        &self.environment.repo
    }
}

impl fmt::Display for DeploymentQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.environment, self.deployment)
    }
}

impl fmt::Debug for DeploymentQid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeploymentQid({self})")
    }
}

impl FromStr for DeploymentQid {
    type Err = ParseIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Split on the last "@" — the deployment ID is always 40 hex chars at the end.
        let Some((env_part, deploy_part)) = s.rsplit_once('@') else {
            return Err(ParseIdError::InvalidDeploymentQid(s.to_string()));
        };
        Ok(Self {
            environment: env_part
                .parse()
                .map_err(|_| ParseIdError::InvalidDeploymentQid(s.to_string()))?,
            deployment: deploy_part
                .parse()
                .map_err(|_| ParseIdError::InvalidDeploymentQid(s.to_string()))?,
        })
    }
}

impl Serialize for DeploymentQid {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DeploymentQid {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_id_valid() {
        assert!("MyOrg".parse::<OrgId>().is_ok());
        assert!("_private".parse::<OrgId>().is_ok());
        assert!("org123".parse::<OrgId>().is_ok());
    }

    #[test]
    fn org_id_invalid() {
        assert!("".parse::<OrgId>().is_err());
        assert!("123abc".parse::<OrgId>().is_err());
        assert!("my-org".parse::<OrgId>().is_err());
        assert!("my.org".parse::<OrgId>().is_err());
    }

    #[test]
    fn repo_qid_roundtrip() {
        let qid: RepoQid = "MyOrg/MyRepo".parse().unwrap();
        assert_eq!(qid.org.as_str(), "MyOrg");
        assert_eq!(qid.repo.as_str(), "MyRepo");
        assert_eq!(qid.to_string(), "MyOrg/MyRepo");
    }

    #[test]
    fn repo_qid_invalid() {
        assert!("just_a_name".parse::<RepoQid>().is_err());
        assert!("a/b/c".parse::<RepoQid>().is_err());
        assert!("/repo".parse::<RepoQid>().is_err());
        assert!("org/".parse::<RepoQid>().is_err());
    }

    #[test]
    fn environment_id_from_git_ref() {
        let env = EnvironmentId::from_git_ref("refs/heads/main").unwrap();
        assert_eq!(env.as_str(), "main");
        assert!(!env.is_tag());
        assert_eq!(env.to_git_ref(), "refs/heads/main");

        let env = EnvironmentId::from_git_ref("refs/heads/feature/my-branch").unwrap();
        assert_eq!(env.as_str(), "feature/my-branch");
        assert_eq!(env.to_git_ref(), "refs/heads/feature/my-branch");

        let env = EnvironmentId::from_git_ref("refs/tags/v1.0").unwrap();
        assert_eq!(env.as_str(), "tag:v1.0");
        assert!(env.is_tag());
        assert_eq!(env.to_git_ref(), "refs/tags/v1.0");
    }

    #[test]
    fn environment_id_from_git_ref_invalid() {
        assert!(EnvironmentId::from_git_ref("main").is_err());
        assert!(EnvironmentId::from_git_ref("refs/heads/").is_err());
        assert!(EnvironmentId::from_git_ref("refs/tags/").is_err());
    }

    #[test]
    fn environment_qid_roundtrip() {
        let qid: EnvironmentQid = "MyOrg/MyRepo::feature/my-branch".parse().unwrap();
        assert_eq!(qid.repo.org.as_str(), "MyOrg");
        assert_eq!(qid.repo.repo.as_str(), "MyRepo");
        assert_eq!(qid.environment.as_str(), "feature/my-branch");
        assert_eq!(qid.to_string(), "MyOrg/MyRepo::feature/my-branch");
    }

    #[test]
    fn deployment_id_valid() {
        let id: DeploymentId = "2cbecbed4bfa1599ef4ce0dfc542c97a82d79268".parse().unwrap();
        assert_eq!(id.as_str(), "2cbecbed4bfa1599ef4ce0dfc542c97a82d79268");
    }

    #[test]
    fn deployment_id_invalid() {
        assert!("too_short".parse::<DeploymentId>().is_err());
        assert!(
            "2CBECBED4BFA1599EF4CE0DFC542C97A82D79268"
                .parse::<DeploymentId>()
                .is_err()
        ); // uppercase
        assert!(
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
                .parse::<DeploymentId>()
                .is_err()
        ); // non-hex
    }

    #[test]
    fn deployment_id_bytes_roundtrip() {
        let id: DeploymentId = "2cbecbed4bfa1599ef4ce0dfc542c97a82d79268".parse().unwrap();
        let bytes = id.to_bytes();
        let id2 = DeploymentId::from_bytes(&bytes).unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn deployment_qid_roundtrip() {
        let s = "MyOrg/MyRepo::main@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268";
        let qid: DeploymentQid = s.parse().unwrap();
        assert_eq!(qid.to_string(), s);
        assert_eq!(qid.environment_qid().to_string(), "MyOrg/MyRepo::main");
        assert_eq!(qid.repo_qid().to_string(), "MyOrg/MyRepo");
        assert_eq!(
            qid.deployment.as_str(),
            "2cbecbed4bfa1599ef4ce0dfc542c97a82d79268"
        );
    }

    #[test]
    fn deployment_qid_with_slashed_env() {
        let s = "MyOrg/MyRepo::feature/my-branch@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268";
        let qid: DeploymentQid = s.parse().unwrap();
        assert_eq!(qid.to_string(), s);
        assert_eq!(
            qid.environment_qid().environment.as_str(),
            "feature/my-branch"
        );
    }

    #[test]
    fn deployment_qid_with_tag_env() {
        let s = "MyOrg/MyRepo::tag:v1.0@2cbecbed4bfa1599ef4ce0dfc542c97a82d79268";
        let qid: DeploymentQid = s.parse().unwrap();
        assert_eq!(qid.to_string(), s);
        assert!(qid.environment_qid().environment.is_tag());
    }

    #[test]
    fn resource_id_roundtrip() {
        let id: ResourceId = "Std/Random.Int:seed".parse().unwrap();
        assert_eq!(id.resource_type(), "Std/Random.Int");
        assert_eq!(id.resource_name(), "seed");
        assert_eq!(id.to_string(), "Std/Random.Int:seed");
    }

    #[test]
    fn resource_id_new() {
        let id = ResourceId::new("Std/Container.Pod", "web");
        assert_eq!(id.resource_type(), "Std/Container.Pod");
        assert_eq!(id.resource_name(), "web");
        assert_eq!(id.to_string(), "Std/Container.Pod:web");
    }

    #[test]
    fn resource_id_with_compound_name() {
        // Resource names can contain slashes (e.g., "pod/container")
        let id = ResourceId::new("Std/Container.Pod.Container", "web/nginx");
        assert_eq!(id.resource_type(), "Std/Container.Pod.Container");
        assert_eq!(id.resource_name(), "web/nginx");
        assert_eq!(id.to_string(), "Std/Container.Pod.Container:web/nginx");
    }

    #[test]
    fn resource_id_parse_compound_name() {
        // The first `:` separates type from name
        let id: ResourceId = "Std/Container.Pod.Container:web/nginx".parse().unwrap();
        assert_eq!(id.resource_type(), "Std/Container.Pod.Container");
        assert_eq!(id.resource_name(), "web/nginx");
    }

    #[test]
    fn resource_id_invalid() {
        assert!("no_colon_separator".parse::<ResourceId>().is_err());
        assert!(":no_type".parse::<ResourceId>().is_err());
        assert!("no_name:".parse::<ResourceId>().is_err());
        assert!("".parse::<ResourceId>().is_err());
    }

    #[test]
    fn resource_qid_roundtrip() {
        let s = "MyOrg/MyRepo::main::Std/Random.Int:seed";
        let qid: ResourceQid = s.parse().unwrap();
        assert_eq!(qid.to_string(), s);
        assert_eq!(qid.environment_qid().to_string(), "MyOrg/MyRepo::main");
        assert_eq!(qid.resource().to_string(), "Std/Random.Int:seed");
        assert_eq!(qid.resource().resource_type(), "Std/Random.Int");
        assert_eq!(qid.resource().resource_name(), "seed");
    }

    #[test]
    fn resource_qid_with_slashed_env() {
        let s = "MyOrg/MyRepo::feature/branch::Std/Artifact.File:readme";
        let qid: ResourceQid = s.parse().unwrap();
        assert_eq!(qid.to_string(), s);
        assert_eq!(
            qid.environment_qid().to_string(),
            "MyOrg/MyRepo::feature/branch"
        );
        assert_eq!(qid.resource().resource_name(), "readme");
    }

    #[test]
    fn resource_qid_invalid() {
        // Missing resource part
        assert!("MyOrg/MyRepo::main".parse::<ResourceQid>().is_err());
        // Invalid resource (no `:` separator within resource)
        assert!("MyOrg/MyRepo::main::nocolon".parse::<ResourceQid>().is_err());
    }

    #[test]
    fn environment_qid_resource_builder() {
        let env_qid: EnvironmentQid = "MyOrg/MyRepo::main".parse().unwrap();
        let resource_id = ResourceId::new("Std/Random.Int", "seed");
        let resource_qid = env_qid.resource(resource_id);
        assert_eq!(
            resource_qid.to_string(),
            "MyOrg/MyRepo::main::Std/Random.Int:seed"
        );
    }
}
