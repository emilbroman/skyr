use std::{fmt, str::FromStr};

use thiserror::Error;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct RepositoryName {
    pub organization: String,
    pub repository: String,
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.organization, self.repository)
    }
}

#[derive(Error, Debug)]
#[error("invalid repository name: {0}")]
pub struct InvalidRepositoryName(String);

impl FromStr for RepositoryName {
    type Err = InvalidRepositoryName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts: Vec<_> = s.split('/').collect();
        if parts.len() != 2 {
            return Err(InvalidRepositoryName(s.to_string()));
        }

        let repository = parts.pop().unwrap().to_string();
        let organization = parts.pop().unwrap().to_string();

        Ok(RepositoryName {
            organization,
            repository,
        })
    }
}
