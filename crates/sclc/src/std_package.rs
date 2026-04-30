use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use ids::ObjId;

use crate::{PackageId, Value};

use super::{DirChild, DirChildKind, LoadError, Package, PackageEntity};

/// A [`Package`] that serves the bundled standard library.
///
/// Wraps the same `include_bytes!` data used by the legacy `StdSourceRepo`.
pub struct StdPackage {
    files: HashMap<String, &'static [u8]>,
}

impl StdPackage {
    pub fn new() -> Self {
        Self {
            files: crate::std::BUNDLED_FILES
                .iter()
                .map(|(path, bytes)| (path.to_string(), *bytes))
                .collect(),
        }
    }

    fn normalize(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }
}

impl Default for StdPackage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Package for StdPackage {
    fn id(&self) -> PackageId {
        PackageId::from(["Std"])
    }

    async fn lookup(&self, path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
        let key = Self::normalize(path);

        // Check if it's a direct file match.
        if let Some(data) = self.files.get(&key) {
            let hash = ObjId::hash_bytes(data);
            return Ok(Some(Cow::Owned(PackageEntity::File { hash })));
        }

        // Check if it's a directory prefix.
        let prefix = if key.is_empty() {
            String::new()
        } else {
            format!("{key}/")
        };

        let mut children: Vec<DirChild> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for (file_key, file_data) in &self.files {
            let relative = if prefix.is_empty() {
                file_key.as_str()
            } else if let Some(rest) = file_key.strip_prefix(&prefix) {
                rest
            } else {
                continue;
            };

            if let Some(slash_pos) = relative.find('/') {
                let dir_name = &relative[..slash_pos];
                if seen.insert(dir_name.to_string()) {
                    children.push(DirChild {
                        name: dir_name.to_string(),
                        kind: DirChildKind::Dir,
                        hash: ObjId::null(),
                    });
                }
            } else if seen.insert(relative.to_string()) {
                children.push(DirChild {
                    name: relative.to_string(),
                    kind: DirChildKind::File,
                    hash: ObjId::hash_bytes(file_data),
                });
            }
        }

        if children.is_empty() {
            Ok(None)
        } else {
            children.sort_by(|a, b| a.name.cmp(&b.name));
            let hash = ObjId::null();
            Ok(Some(Cow::Owned(PackageEntity::Dir { hash, children })))
        }
    }

    async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
        let key = Self::normalize(path);
        match self.files.get(&key) {
            Some(data) => Ok(Cow::Owned(data.to_vec())),
            None => Err(LoadError::NotFound(key)),
        }
    }

    fn register_externs(&self, externs: &mut HashMap<String, Value>) {
        externs.extend(crate::std::collect_std_externs());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::PackageFinder;

    #[tokio::test]
    async fn lookup_existing_file() {
        let pkg = StdPackage::new();
        let result = pkg.lookup(Path::new("Time.scl")).await.unwrap();
        assert!(result.is_some());
        assert!(matches!(
            result.unwrap().as_ref(),
            PackageEntity::File { .. }
        ));
    }

    #[tokio::test]
    async fn lookup_nonexistent_file() {
        let pkg = StdPackage::new();
        let result = pkg.lookup(Path::new("NonExistent.scl")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn lookup_root_dir() {
        let pkg = StdPackage::new();
        let result = pkg.lookup(Path::new("")).await.unwrap();
        assert!(result.is_some());
        if let PackageEntity::Dir { children, .. } = result.unwrap().as_ref() {
            let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
            assert!(names.contains(&"Time.scl"));
        } else {
            panic!("expected Dir");
        }
    }

    #[tokio::test]
    async fn load_existing_file() {
        let pkg = StdPackage::new();
        let data = pkg.load(Path::new("Time.scl")).await.unwrap();
        assert!(!data.is_empty());
    }

    #[tokio::test]
    async fn load_nonexistent_file() {
        let pkg = StdPackage::new();
        let result = pkg.load(Path::new("NonExistent.scl")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn as_package_finder_matching() {
        let pkg: Arc<StdPackage> = Arc::new(StdPackage::new());
        let result = pkg.find(&["Std", "Time"]).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id(), PackageId::from(["Std"]));
    }

    #[tokio::test]
    async fn as_package_finder_non_matching() {
        let pkg: Arc<StdPackage> = Arc::new(StdPackage::new());
        let result = pkg.find(&["Other", "Time"]).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn register_externs_provides_values() {
        let pkg = StdPackage::new();
        let mut externs = HashMap::new();
        pkg.register_externs(&mut externs);
        // Should have at least some extern functions registered.
        assert!(!externs.is_empty());
    }
}
