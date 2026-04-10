use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::{PackageId, Value};

/// A filesystem-like entity within a [`Package`].
#[derive(Clone, Debug)]
pub enum PackageEntity {
    File {
        hash: gix_hash::ObjectId,
    },
    Dir {
        hash: gix_hash::ObjectId,
        children: Vec<DirChild>,
    },
}

/// A child entry within a [`PackageEntity::Dir`].
#[derive(Clone, Debug)]
pub struct DirChild {
    pub name: String,
    pub kind: DirChildKind,
    pub hash: gix_hash::ObjectId,
}

/// Whether a [`DirChild`] is a file or directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirChildKind {
    File,
    Dir,
}

/// Errors that can occur when loading from a [`Package`].
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("encoding error: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),

    #[error("{0}")]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// A package that can look up and load files.
///
/// Object-safe trait representing a package. Implementations include
/// `StdPackage` (bundled stdlib), `InMemoryPackage` (for tests/LSP),
/// `FsPackage` (filesystem), and CDB-backed packages.
#[async_trait::async_trait]
pub trait Package: Send + Sync {
    /// Returns the package identifier (e.g. `Std`, `MyOrg/MyRepo`).
    fn id(&self) -> PackageId;

    /// Looks up a filesystem entity at the given path within this package.
    ///
    /// Returns `Ok(None)` if the path does not exist, or `Err` on I/O failure.
    async fn lookup(&self, path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError>;

    /// Loads the raw bytes of a file at the given path.
    ///
    /// Unlike [`lookup`](Package::lookup), a missing file is an error here.
    async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError>;

    /// Registers native (Rust) extern function implementations provided by this package.
    ///
    /// Most packages return nothing; `StdPackage` uses this to register stdlib externs.
    fn register_externs(&self, _externs: &mut HashMap<String, Value>) {}
}

/// Finds the [`Package`] that owns a given raw module ID.
///
/// The raw module ID is an unresolved slice of path segments (e.g.
/// `["MyOrg", "MyRepo", "Foo", "Bar"]`). The finder returns a package whose
/// [`Package::id`] matches the leading segments of the raw ID.
#[async_trait::async_trait]
pub trait PackageFinder: Send + Sync {
    async fn find(&self, raw_id: &[&str]) -> Result<Option<Arc<dyn Package>>, LoadError>;
}

/// Blanket impl: any `Arc<P: Package>` can act as a `PackageFinder` by checking
/// whether the raw module ID starts with its own package ID segments.
#[async_trait::async_trait]
impl<P: Package + 'static> PackageFinder for Arc<P> {
    async fn find(&self, raw_id: &[&str]) -> Result<Option<Arc<dyn Package>>, LoadError> {
        let pkg_id = self.id();
        let segments = pkg_id.as_slice();
        if raw_id.len() >= segments.len()
            && raw_id[..segments.len()]
                .iter()
                .zip(segments.iter())
                .all(|(a, b)| *a == b.as_str())
        {
            Ok(Some(Arc::clone(self) as Arc<dyn Package>))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal test package for unit tests.
    struct TestPackage {
        pkg_id: PackageId,
    }

    #[async_trait::async_trait]
    impl Package for TestPackage {
        fn id(&self) -> PackageId {
            self.pkg_id.clone()
        }

        async fn lookup(&self, _path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
            Ok(None)
        }

        async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
            Err(LoadError::NotFound(path.display().to_string()))
        }
    }

    #[test]
    fn package_entity_construction() {
        let file = PackageEntity::File {
            hash: gix_hash::ObjectId::null(gix_hash::Kind::Sha1),
        };
        assert!(matches!(file, PackageEntity::File { .. }));

        let dir = PackageEntity::Dir {
            hash: gix_hash::ObjectId::null(gix_hash::Kind::Sha1),
            children: vec![DirChild {
                name: "foo.scl".into(),
                kind: DirChildKind::File,
                hash: gix_hash::ObjectId::null(gix_hash::Kind::Sha1),
            }],
        };
        if let PackageEntity::Dir { children, .. } = &dir {
            assert_eq!(children.len(), 1);
            assert_eq!(children[0].name, "foo.scl");
            assert_eq!(children[0].kind, DirChildKind::File);
        } else {
            panic!("expected Dir");
        }
    }

    #[tokio::test]
    async fn arc_package_finder_matching() {
        let pkg = Arc::new(TestPackage {
            pkg_id: PackageId::from(["Std"]),
        });

        // Should match: raw ID starts with package ID
        let result = pkg.find(&["Std", "Time"]).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id(), PackageId::from(["Std"]));
    }

    #[tokio::test]
    async fn arc_package_finder_non_matching() {
        let pkg = Arc::new(TestPackage {
            pkg_id: PackageId::from(["Std"]),
        });

        // Should not match: different prefix
        let result = pkg.find(&["Other", "Time"]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn arc_package_finder_multi_segment() {
        let pkg = Arc::new(TestPackage {
            pkg_id: PackageId::from(["MyOrg", "MyRepo"]),
        });

        // Should match: raw ID starts with both segments
        let result = pkg.find(&["MyOrg", "MyRepo", "Foo", "Bar"]).await.unwrap();
        assert!(result.is_some());

        // Should not match: only first segment matches
        let result = pkg.find(&["MyOrg", "Other"]).await.unwrap();
        assert!(result.is_none());

        // Should not match: raw ID too short
        let result = pkg.find(&["MyOrg"]).await.unwrap();
        assert!(result.is_none());
    }
}
