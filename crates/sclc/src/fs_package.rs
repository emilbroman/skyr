use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{PackageId, Value};

use super::{DirChild, DirChildKind, LoadError, Package, PackageEntity};

/// A filesystem-backed [`Package`].
///
/// Requires the `fs` feature (which enables `tokio/fs`).
#[derive(Clone)]
pub struct FsPackage {
    root: PathBuf,
    package_id: PackageId,
}

impl FsPackage {
    pub fn new(root: PathBuf, package_id: PackageId) -> Self {
        Self { root, package_id }
    }
}

#[async_trait::async_trait]
impl Package for FsPackage {
    fn id(&self) -> PackageId {
        self.package_id.clone()
    }

    async fn lookup(&self, path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
        let full = self.root.join(path);
        let metadata = match tokio::fs::metadata(&full).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(LoadError::Io(e)),
        };

        let null_hash = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);

        if metadata.is_file() {
            Ok(Some(Cow::Owned(PackageEntity::File { hash: null_hash })))
        } else if metadata.is_dir() {
            let mut read_dir = tokio::fs::read_dir(&full).await?;
            let mut children = Vec::new();
            while let Some(entry) = read_dir.next_entry().await? {
                let ft = entry.file_type().await?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let kind = if ft.is_dir() {
                    DirChildKind::Dir
                } else {
                    DirChildKind::File
                };
                children.push(DirChild {
                    name,
                    kind,
                    hash: null_hash,
                });
            }
            children.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(Some(Cow::Owned(PackageEntity::Dir {
                hash: null_hash,
                children,
            })))
        } else {
            Ok(None)
        }
    }

    async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
        let full = self.root.join(path);
        match tokio::fs::read(&full).await {
            Ok(data) => Ok(Cow::Owned(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(LoadError::NotFound(path.display().to_string()))
            }
            Err(e) => Err(LoadError::Io(e)),
        }
    }

    fn register_externs(&self, _externs: &mut HashMap<String, Value>) {}

    fn root_path(&self) -> Option<&Path> {
        Some(&self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fs_package_basic_operations() {
        let dir = std::env::temp_dir().join("sclc_v2_fs_package_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("Main.scl"), b"let x = 1").unwrap();
        std::fs::write(dir.join("sub/Foo.scl"), b"let y = 2").unwrap();

        let pkg = FsPackage::new(dir.clone(), PackageId::from(["Test"]));

        // lookup file
        let result = pkg.lookup(Path::new("Main.scl")).await.unwrap();
        assert!(matches!(
            result.unwrap().as_ref(),
            PackageEntity::File { .. }
        ));

        // lookup dir
        let result = pkg.lookup(Path::new("sub")).await.unwrap();
        assert!(matches!(
            result.unwrap().as_ref(),
            PackageEntity::Dir { .. }
        ));

        // lookup missing
        let result = pkg.lookup(Path::new("Missing.scl")).await.unwrap();
        assert!(result.is_none());

        // load file
        let data = pkg.load(Path::new("Main.scl")).await.unwrap();
        assert_eq!(data.as_ref(), b"let x = 1");

        // load missing
        assert!(pkg.load(Path::new("Missing.scl")).await.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
