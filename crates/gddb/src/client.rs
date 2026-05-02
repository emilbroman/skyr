use std::sync::Arc;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use ids::{OrgId, RegionId, RepoQid, name_hash};
use scylla::{
    client::{session::Session, session_builder::SessionBuilder},
    errors::PrepareError,
    statement::prepared::PreparedStatement,
};

use crate::error::{ConnectError, LookupError, ReserveError, UpsertError};

/// A region's identity-token signing public key, as stored in GDDB.
///
/// `public_key` bytes are opaque to GDDB — interpretation belongs to
/// `auth_token`. `generation` increments on every rotation; callers can use
/// it to invalidate caches when they observe a higher generation than they
/// have on hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionKey {
    pub public_key: Vec<u8>,
    pub generation: i32,
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
        create_org_names_table = r#"
            CREATE TABLE IF NOT EXISTS gddb.org_names (
                name_hash BLOB PRIMARY KEY,
                name TEXT,
                region TEXT,
                created_at TIMESTAMP
            )
        "#,

        create_repo_names_table = r#"
            CREATE TABLE IF NOT EXISTS gddb.repo_names (
                name_hash BLOB PRIMARY KEY,
                org TEXT,
                repo TEXT,
                region TEXT,
                created_at TIMESTAMP
            )
        "#,

        create_region_keys_table = r#"
            CREATE TABLE IF NOT EXISTS gddb.region_keys (
                region TEXT PRIMARY KEY,
                public_key BLOB,
                generation INT,
                updated_at TIMESTAMP
            )
        "#,
    }

    PreparedStatements {
        reserve_org = r#"
            INSERT INTO gddb.org_names (name_hash, name, region, created_at)
            VALUES (?, ?, ?, ?)
            IF NOT EXISTS
        "#,

        reserve_repo = r#"
            INSERT INTO gddb.repo_names (name_hash, org, repo, region, created_at)
            VALUES (?, ?, ?, ?, ?)
            IF NOT EXISTS
        "#,

        lookup_org = r#"
            SELECT region FROM gddb.org_names
            WHERE name_hash = ?
        "#,

        lookup_repo = r#"
            SELECT region FROM gddb.repo_names
            WHERE name_hash = ?
        "#,

        upsert_region_key = r#"
            INSERT INTO gddb.region_keys (region, public_key, generation, updated_at)
            VALUES (?, ?, ?, ?)
        "#,

        lookup_region_key = r#"
            SELECT public_key, generation FROM gddb.region_keys
            WHERE region = ?
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
            "CREATE KEYSPACE IF NOT EXISTS gddb \
             WITH replication = {{'class': 'SimpleStrategy', 'replication_factor': {}}}",
            self.replication_factor,
        );
        let create_keyspace = session.prepare(create_keyspace_cql).await?;
        session.execute_unpaged(&create_keyspace, ()).await?;

        let table_statements = TableStatements::new(&session).await?;
        let (r0, r1, r2) = futures::join!(
            session.execute_unpaged(&table_statements.create_org_names_table, ()),
            session.execute_unpaged(&table_statements.create_repo_names_table, ()),
            session.execute_unpaged(&table_statements.create_region_keys_table, ()),
        );
        r0?;
        r1?;
        r2?;

        let statements = PreparedStatements::new(&session).await?;

        Ok(Client {
            session,
            statements,
        })
    }
}

#[derive(Clone)]
pub struct Client {
    session: Arc<Session>,
    statements: PreparedStatements,
}

impl Client {
    /// Reserve `org` for `region`. Atomic via LWT (`INSERT IF NOT EXISTS` on
    /// `sha1(lower(org))`). Returns `NameTaken` if the hash is already
    /// claimed — including by a differently-cased spelling of the same
    /// name.
    pub async fn reserve_org(&self, org: &OrgId, region: &RegionId) -> Result<(), ReserveError> {
        let hash = name_hash(org.as_str());
        let now = Utc::now();
        let result = self
            .session
            .execute_unpaged(
                &self.statements.reserve_org,
                (hash.as_slice(), org.as_str(), region.as_str(), now),
            )
            .await?;

        // LWT result rows always start with `[applied]` as the first column.
        let rows = result.into_rows_result()?;
        type ApplyRow = (
            bool,
            Option<Vec<u8>>,       // existing name_hash
            Option<DateTime<Utc>>, // existing created_at
            Option<String>,        // existing name
            Option<String>,        // existing region
        );
        let row = rows.first_row::<ApplyRow>()?;
        if !row.0 {
            return Err(ReserveError::NameTaken);
        }
        Ok(())
    }

    /// Reserve `org/repo` for `region`. Atomic via LWT.
    pub async fn reserve_repo(&self, qid: &RepoQid, region: &RegionId) -> Result<(), ReserveError> {
        let hash = name_hash(&qid.to_string());
        let now = Utc::now();
        let result = self
            .session
            .execute_unpaged(
                &self.statements.reserve_repo,
                (
                    hash.as_slice(),
                    qid.org.as_str(),
                    qid.repo.as_str(),
                    region.as_str(),
                    now,
                ),
            )
            .await?;

        let rows = result.into_rows_result()?;
        type ApplyRow = (
            bool,
            Option<Vec<u8>>,       // existing name_hash
            Option<DateTime<Utc>>, // existing created_at
            Option<String>,        // existing org
            Option<String>,        // existing region
            Option<String>,        // existing repo
        );
        let row = rows.first_row::<ApplyRow>()?;
        if !row.0 {
            return Err(ReserveError::NameTaken);
        }
        Ok(())
    }

    /// Look up the home region of an org. Returns `None` if the name is
    /// not reserved.
    pub async fn lookup_org(&self, org: &OrgId) -> Result<Option<RegionId>, LookupError> {
        let hash = name_hash(org.as_str());
        self.lookup_region(&self.statements.lookup_org, hash.as_slice())
            .await
    }

    /// Look up the home region of an `org/repo`. Returns `None` if the name
    /// is not reserved.
    pub async fn lookup_repo(&self, qid: &RepoQid) -> Result<Option<RegionId>, LookupError> {
        let hash = name_hash(&qid.to_string());
        self.lookup_region(&self.statements.lookup_repo, hash.as_slice())
            .await
    }

    /// Publish `region`'s identity-token signing public key, replacing any
    /// previously-stored key for the same region. `generation` lets readers
    /// detect rotations: bump it whenever `public_key` changes.
    ///
    /// Idempotent — safe to call on every UDB startup.
    pub async fn upsert_region_key(
        &self,
        region: &RegionId,
        public_key: &[u8],
        generation: i32,
    ) -> Result<(), UpsertError> {
        let now = Utc::now();
        self.session
            .execute_unpaged(
                &self.statements.upsert_region_key,
                (region.as_str(), public_key, generation, now),
            )
            .await?;
        Ok(())
    }

    /// Look up `region`'s identity-token signing public key. Returns `None`
    /// if the region has not registered a key yet.
    pub async fn lookup_region_key(
        &self,
        region: &RegionId,
    ) -> Result<Option<RegionKey>, LookupError> {
        let pager = self
            .session
            .execute_iter(
                self.statements.lookup_region_key.clone(),
                (region.as_str(),),
            )
            .await?;

        let mut stream = pager.rows_stream::<(Vec<u8>, i32)>()?;
        match stream.try_next().await? {
            None => Ok(None),
            Some((public_key, generation)) => Ok(Some(RegionKey {
                public_key,
                generation,
            })),
        }
    }

    async fn lookup_region(
        &self,
        statement: &PreparedStatement,
        name_hash: &[u8],
    ) -> Result<Option<RegionId>, LookupError> {
        let pager = self
            .session
            .execute_iter(statement.clone(), (name_hash,))
            .await?;

        let mut stream = pager.rows_stream::<(String,)>()?;
        match stream.try_next().await? {
            None => Ok(None),
            Some((region,)) => Ok(Some(region.parse()?)),
        }
    }
}
