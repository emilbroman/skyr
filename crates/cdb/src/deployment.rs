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
    Desired,
    Up,
}

impl fmt::Display for DeploymentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Down => write!(f, "DOWN"),
            Self::Undesired => write!(f, "UNDESIRED"),
            Self::Lingering => write!(f, "LINGERING"),
            Self::Desired => write!(f, "DESIRED"),
            Self::Up => write!(f, "UP"),
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
            "DESIRED" => Ok(DeploymentState::Desired),
            "UP" => Ok(DeploymentState::Up),
            v => Err(InvalidDeploymentState(v.to_string())),
        }
    }
}
