use std::error::Error;
use std::path::Path;

use std::collections::HashMap;

use crate::{PackageId, Value};

/// A boxed, thread-safe error type used by [`SourceRepo`] methods.
pub type SourceError = Box<dyn Error + Send + Sync>;

/// An entry returned by [`SourceRepo::list_children`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChildEntry {
    /// A file (filename with extension).
    File(String),
    /// A subdirectory.
    Directory(String),
}

impl ChildEntry {
    /// Return the name of this entry.
    pub fn name(&self) -> &str {
        match self {
            ChildEntry::File(name) | ChildEntry::Directory(name) => name,
        }
    }

    /// If this is an `.scl` file, return the module name (stem without extension).
    pub fn as_module(&self) -> Option<&str> {
        match self {
            ChildEntry::File(name) => name.strip_suffix(".scl"),
            ChildEntry::Directory(_) => None,
        }
    }
}

#[async_trait::async_trait]
pub trait SourceRepo: Send + Sync {
    fn package_id(&self) -> PackageId;
    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, SourceError>;

    /// List child entries (modules, directories, and files) under the given path
    /// within this source repository.
    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, SourceError> {
        let _ = path;
        Ok(Vec::new())
    }

    /// Return the Git object hash for a path in this source repository,
    /// or `None` if the path does not exist.
    ///
    /// The default implementation returns a null SHA-1 hash when the path
    /// exists (sufficient for non-CDB backends). CDB overrides this with
    /// the real tree/blob OID.
    async fn path_hash(&self, path: &Path) -> Result<Option<gix_hash::ObjectId>, SourceError> {
        Ok(self
            .read_file(path)
            .await?
            .map(|_| gix_hash::ObjectId::null(gix_hash::Kind::Sha1)))
    }

    /// Return extern function implementations provided by this source.
    /// Default: no externs. StdSourceRepo overrides this with std library impls.
    fn externs(&self) -> HashMap<String, Value> {
        HashMap::new()
    }
}

#[cfg(feature = "cdb")]
#[async_trait::async_trait]
impl SourceRepo for cdb::DeploymentClient {
    fn package_id(&self) -> PackageId {
        let repo_qid = self.repo_qid();
        [repo_qid.org.to_string(), repo_qid.repo.to_string()]
            .into_iter()
            .collect::<PackageId>()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, SourceError> {
        match self.read_file(path).await {
            Ok(data) => Ok(Some(data)),
            Err(cdb::FileError::NotFound(_)) => Ok(None),
            Err(source) => Err(source.into()),
        }
    }

    async fn path_hash(&self, path: &Path) -> Result<Option<gix_hash::ObjectId>, SourceError> {
        self.path_hash(path).await.map_err(Into::into)
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, SourceError> {
        let dir_path = if path.as_os_str().is_empty() {
            None
        } else {
            Some(path)
        };
        match self.read_dir(dir_path).await {
            Ok(tree) => {
                let mut entries = Vec::new();
                for entry in &tree.entries {
                    let name = String::from_utf8_lossy(&entry.filename).into_owned();
                    if entry.mode.is_tree() {
                        entries.push(ChildEntry::Directory(name));
                    } else if entry.mode.is_blob() {
                        entries.push(ChildEntry::File(name));
                    }
                }
                Ok(entries)
            }
            Err(cdb::FileError::NotFound(_)) => Ok(Vec::new()),
            Err(err) => Err(err.into()),
        }
    }
}
