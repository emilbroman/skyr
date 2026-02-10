use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use gix_hash::ObjectId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::repository_name::RepositoryName;

#[derive(Clone, Debug)]
pub struct Deployment {
    pub repository: RepositoryName,
    pub id: DeploymentId,
    pub created_at: DateTime<Utc>,
    pub state: DeploymentState,
}

impl Deployment {
    pub fn fqid(&self) -> String {
        format!("{}/{}", self.repository, self.id)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DeploymentId {
    pub ref_name: String,
    pub commit_hash: ObjectId,
}

impl fmt::Display for DeploymentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.ref_name, self.commit_hash)
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeploymentState {
    Down,
    Undesired,
    Lingering,
    Desired,
}

impl fmt::Display for DeploymentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Down => write!(f, "DOWN"),
            Self::Undesired => write!(f, "UNDESIRED"),
            Self::Lingering => write!(f, "LINGERING"),
            Self::Desired => write!(f, "DESIRED"),
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
            v => Err(InvalidDeploymentState(v.to_string())),
        }
    }
}
