use chrono::{DateTime, Utc};
use ids::RepoQid;

#[derive(Clone, Debug)]
pub struct Repository {
    pub name: RepoQid,
    pub created_at: DateTime<Utc>,
}
