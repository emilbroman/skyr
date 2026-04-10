use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::{ChildEntry, PackageId, SourceError, SourceRepo};

/// An in-memory source repository backed by a map of file paths to contents.
///
/// Keys in the `files` map use forward-slash separators (e.g. `"Main.scl"`,
/// `"subdir/Foo.scl"`).
#[derive(Clone)]
pub struct MemSourceRepo {
    package_id: PackageId,
    files: HashMap<String, Vec<u8>>,
}

impl MemSourceRepo {
    pub fn new(package_id: PackageId, files: HashMap<String, Vec<u8>>) -> Self {
        Self { package_id, files }
    }

    /// Consume the repo and return the internal files map.
    pub fn into_files(self) -> HashMap<String, Vec<u8>> {
        self.files
    }
}

#[async_trait::async_trait]
impl SourceRepo for MemSourceRepo {
    fn package_id(&self) -> PackageId {
        self.package_id.clone()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, SourceError> {
        let key = path.to_string_lossy().replace('\\', "/");
        Ok(self.files.get(&key).cloned())
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, SourceError> {
        let prefix = {
            let raw = path.to_string_lossy().replace('\\', "/");
            if raw.is_empty() {
                String::new()
            } else {
                format!("{raw}/")
            }
        };
        let mut entries = BTreeSet::new();
        for key in self.files.keys() {
            let relative = if prefix.is_empty() {
                key.as_str()
            } else if let Some(rest) = key.strip_prefix(&prefix) {
                rest
            } else {
                continue;
            };
            // Only direct children (first path segment of `relative`).
            if let Some(slash_pos) = relative.find('/') {
                entries.insert(ChildEntry::Directory(relative[..slash_pos].to_owned()));
            } else {
                entries.insert(ChildEntry::File(relative.to_owned()));
            }
        }
        Ok(entries.into_iter().collect())
    }
}
