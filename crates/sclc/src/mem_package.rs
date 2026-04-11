use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::PackageId;

use super::{DirChild, DirChildKind, LoadError, Package, PackageEntity};

/// An in-memory [`Package`] backed by a `RwLock<HashMap>`.
///
/// Supports mutation via `&self` for use behind `Arc` (e.g. in the LSP).
pub struct InMemoryPackage {
    id: PackageId,
    files: RwLock<HashMap<PathBuf, Vec<u8>>>,
}

impl InMemoryPackage {
    pub fn new(id: PackageId, files: HashMap<PathBuf, Vec<u8>>) -> Self {
        Self {
            id,
            files: RwLock::new(files),
        }
    }

    pub fn empty(id: PackageId) -> Self {
        Self::new(id, HashMap::new())
    }

    /// Insert or update a file. Takes `&self` via interior mutability.
    pub fn update_file(&self, path: PathBuf, content: Vec<u8>) {
        self.files.write().unwrap().insert(path, content);
    }

    /// Remove a file. Takes `&self` via interior mutability.
    pub fn remove_file(&self, path: &Path) {
        self.files.write().unwrap().remove(path);
    }
}

#[async_trait::async_trait]
impl Package for InMemoryPackage {
    fn id(&self) -> PackageId {
        self.id.clone()
    }

    async fn lookup(&self, path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
        let files = self.files.read().unwrap();
        let normalized = normalize(path);

        // Direct file match.
        if let Some(data) = files.get(&normalized) {
            let hash = hash_bytes(data);
            return Ok(Some(Cow::Owned(PackageEntity::File { hash })));
        }

        // Directory prefix match.
        let prefix = if normalized.as_os_str().is_empty() {
            PathBuf::new()
        } else {
            normalized.clone()
        };

        let mut children: Vec<DirChild> = Vec::new();
        let mut seen = HashSet::new();

        for (file_path, file_data) in files.iter() {
            let relative = if prefix.as_os_str().is_empty() {
                file_path.as_path()
            } else if let Ok(rest) = file_path.strip_prefix(&prefix) {
                rest
            } else {
                continue;
            };

            let mut components = relative.components();
            if let Some(first) = components.next() {
                let name = first.as_os_str().to_string_lossy().to_string();
                if components.next().is_some() {
                    // It's a subdirectory entry.
                    if seen.insert(name.clone()) {
                        children.push(DirChild {
                            name,
                            kind: DirChildKind::Dir,
                            hash: gix_hash::ObjectId::null(gix_hash::Kind::Sha1),
                        });
                    }
                } else if seen.insert(name.clone()) {
                    children.push(DirChild {
                        name,
                        kind: DirChildKind::File,
                        hash: hash_bytes(file_data),
                    });
                }
            }
        }

        if children.is_empty() {
            Ok(None)
        } else {
            children.sort_by(|a, b| a.name.cmp(&b.name));
            let hash = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);
            Ok(Some(Cow::Owned(PackageEntity::Dir { hash, children })))
        }
    }

    async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
        let files = self.files.read().unwrap();
        let normalized = normalize(path);
        match files.get(&normalized) {
            Some(data) => Ok(Cow::Owned(data.clone())),
            None => Err(LoadError::NotFound(path.display().to_string())),
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    // Normalize backslashes for cross-platform consistency.
    PathBuf::from(path.to_string_lossy().replace('\\', "/"))
}

fn hash_bytes(data: &[u8]) -> gix_hash::ObjectId {
    use sha1::Digest;
    let hash = sha1::Sha1::digest(data);
    gix_hash::ObjectId::from_bytes_or_panic(&hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_package() -> InMemoryPackage {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"let x = 1".to_vec());
        files.insert(PathBuf::from("Sub/Foo.scl"), b"let y = 2".to_vec());
        InMemoryPackage::new(PackageId::from(["Test"]), files)
    }

    #[tokio::test]
    async fn lookup_file() {
        let pkg = test_package();
        let result = pkg.lookup(Path::new("Main.scl")).await.unwrap();
        assert!(matches!(
            result.unwrap().as_ref(),
            PackageEntity::File { .. }
        ));
    }

    #[tokio::test]
    async fn lookup_dir() {
        let pkg = test_package();
        let result = pkg.lookup(Path::new("Sub")).await.unwrap();
        if let PackageEntity::Dir { children, .. } = result.unwrap().as_ref() {
            assert_eq!(children.len(), 1);
            assert_eq!(children[0].name, "Foo.scl");
            assert_eq!(children[0].kind, DirChildKind::File);
        } else {
            panic!("expected Dir");
        }
    }

    #[tokio::test]
    async fn lookup_root() {
        let pkg = test_package();
        let result = pkg.lookup(Path::new("")).await.unwrap();
        if let PackageEntity::Dir { children, .. } = result.unwrap().as_ref() {
            let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
            assert!(names.contains(&"Main.scl"));
            assert!(names.contains(&"Sub"));
        } else {
            panic!("expected Dir");
        }
    }

    #[tokio::test]
    async fn lookup_nonexistent() {
        let pkg = test_package();
        let result = pkg.lookup(Path::new("Missing.scl")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn load_file() {
        let pkg = test_package();
        let data = pkg.load(Path::new("Main.scl")).await.unwrap();
        assert_eq!(data.as_ref(), b"let x = 1");
    }

    #[tokio::test]
    async fn load_missing() {
        let pkg = test_package();
        assert!(pkg.load(Path::new("Missing.scl")).await.is_err());
    }

    #[tokio::test]
    async fn update_and_remove() {
        let pkg = InMemoryPackage::empty(PackageId::from(["Test"]));
        assert!(pkg.load(Path::new("A.scl")).await.is_err());

        pkg.update_file(PathBuf::from("A.scl"), b"hello".to_vec());
        assert_eq!(
            pkg.load(Path::new("A.scl")).await.unwrap().as_ref(),
            b"hello"
        );

        pkg.remove_file(Path::new("A.scl"));
        assert!(pkg.load(Path::new("A.scl")).await.is_err());
    }
}
