use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use ids::{DeploymentId, DeploymentQid, EnvironmentId, EnvironmentQid, RepoQid};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A deployment is a revision of an environment. It represents a specific commit
/// being applied to a particular environment (Git ref) within a repository.
#[derive(Clone, Debug)]
pub struct Deployment {
    /// The repository this deployment belongs to.
    pub repo: RepoQid,
    /// The environment (Git ref) this deployment targets.
    pub environment: EnvironmentId,
    /// The deployment ID (commit hash).
    pub deployment: DeploymentId,
    /// When this deployment was created.
    pub created_at: DateTime<Utc>,
    /// Current lifecycle state.
    pub state: DeploymentState,
}

impl Deployment {
    /// Returns the fully qualified deployment identifier.
    pub fn deployment_qid(&self) -> DeploymentQid {
        self.environment_qid().deployment(self.deployment.clone())
    }

    /// Returns the fully qualified environment identifier.
    pub fn environment_qid(&self) -> EnvironmentQid {
        self.repo.environment(self.environment.clone())
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeploymentState {
    Down,
    Undesired,
    Lingering,
    /// Initial state for a freshly-created deployment. The deployment's own
    /// worker promotes it to `Desired` once any predecessor it supersedes
    /// has acknowledged the supersession (i.e. reached `Lingering`).
    Pending,
    Desired,
    Up,
    Failing,
    Failed,
}

impl DeploymentState {
    /// Whether a deployment in this state is a "live" lifecycle state —
    /// i.e. not yet terminal (`Down`, `Undesired`, `Failed`) and not
    /// waiting for teardown (`Lingering`).
    ///
    /// In the redesigned state machine, "which deployment is the tip of an
    /// environment" is determined by supersession-absence rather than this
    /// predicate, so callers should generally prefer checking the
    /// `supersessions` table directly.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            DeploymentState::Pending
                | DeploymentState::Desired
                | DeploymentState::Up
                | DeploymentState::Failing
        )
    }
}

impl fmt::Display for DeploymentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Down => write!(f, "DOWN"),
            Self::Undesired => write!(f, "UNDESIRED"),
            Self::Lingering => write!(f, "LINGERING"),
            Self::Pending => write!(f, "PENDING"),
            Self::Desired => write!(f, "DESIRED"),
            Self::Up => write!(f, "UP"),
            Self::Failing => write!(f, "FAILING"),
            Self::Failed => write!(f, "FAILED"),
        }
    }
}

#[derive(Error, Debug)]
#[error("invalid deployment state: {0}")]
pub struct InvalidDeploymentState(String);

impl FromStr for DeploymentState {
    type Err = InvalidDeploymentState;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "DOWN" => Ok(DeploymentState::Down),
            "UNDESIRED" => Ok(DeploymentState::Undesired),
            "LINGERING" => Ok(DeploymentState::Lingering),
            "PENDING" => Ok(DeploymentState::Pending),
            "DESIRED" => Ok(DeploymentState::Desired),
            "UP" => Ok(DeploymentState::Up),
            "FAILING" => Ok(DeploymentState::Failing),
            "FAILED" => Ok(DeploymentState::Failed),
            v => Err(InvalidDeploymentState(v.to_string())),
        }
    }
}
