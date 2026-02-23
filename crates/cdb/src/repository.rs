use chrono::{DateTime, Utc};

use crate::repository_name::RepositoryName;

#[derive(Clone, Debug)]
pub struct Repository {
    pub name: RepositoryName,
    pub created_at: DateTime<Utc>,
}
