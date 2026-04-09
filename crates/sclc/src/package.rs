use std::path::{Component, Path};
use std::sync::Arc;
use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

use crate::{ChildEntry, SourceError, SourceRepo};

#[derive(Clone)]
pub struct Package {
    source: Arc<dyn SourceRepo>,
    /// Cached directory listings, keyed by the directory path within the package.
    children_cache: HashMap<PathBuf, Vec<ChildEntry>>,
}

#[derive(Error, Debug)]
pub enum OpenError {
    #[error("module not found: {0}")]
    NotFound(PathBuf),

    #[error("path traversal rejected: {0}")]
    PathTraversal(PathBuf),

    #[error("failed to load source file: {0}")]
    Source(#[from] SourceError),

    #[error("encoding error: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),
}

impl Package {
    pub fn new(source: Arc<dyn SourceRepo>) -> Self {
        Self {
            source,
            children_cache: HashMap::new(),
        }
    }

    pub fn replace_source(&mut self, source: Arc<dyn SourceRepo>) {
        self.source = source;
        self.children_cache.clear();
    }

    /// Synchronously look up previously cached children for a path.
    pub fn cached_children(&self, path: &Path) -> Option<&[ChildEntry]> {
        self.children_cache.get(path).map(Vec::as_slice)
    }

    /// Iterate over all cached children entries.
    pub fn children_entries(&self) -> impl Iterator<Item = (&PathBuf, &Vec<ChildEntry>)> {
        self.children_cache.iter()
    }

    /// Access the underlying source repo.
    pub fn source(&self) -> &dyn SourceRepo {
        &*self.source
    }

    pub fn package_id(&self) -> crate::PackageId {
        self.source.package_id()
    }

    pub async fn read_module_source(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<Option<String>, OpenError> {
        let path = path.as_ref().to_path_buf();

        // Reject paths that contain traversal components (e.g. ".." or
        // absolute prefixes) to prevent escaping the package directory.
        for component in path.components() {
            match component {
                Component::Normal(_) => {}
                _ => return Err(OpenError::PathTraversal(path)),
            }
        }

        let source_data = self.source.read_file(&path).await?;
        let Some(source_data) = source_data else {
            return Ok(None);
        };
        Ok(Some(String::from_utf8(source_data)?))
    }

    /// List child entries at the given path, caching the result.
    pub async fn list_children(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Vec<ChildEntry>, OpenError> {
        let path = path.as_ref().to_path_buf();
        if let Some(cached) = self.children_cache.get(&path) {
            return Ok(cached.clone());
        }
        let entries = self.source.list_children(&path).await?;
        self.children_cache.insert(path, entries.clone());
        Ok(entries)
    }
}
