use std::path::{Path, PathBuf};

use crate::{ChildEntry, ModuleId, SourceError, SourceRepo};

/// A filesystem-backed source repository.
///
/// Requires the `fs` feature (which enables `tokio/fs`).
#[derive(Clone)]
pub struct FsSource {
    pub root: PathBuf,
    pub package_id: ModuleId,
}

#[async_trait::async_trait]
impl SourceRepo for FsSource {
    fn package_id(&self) -> ModuleId {
        self.package_id.clone()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, SourceError> {
        match tokio::fs::read(self.root.join(path)).await {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, SourceError> {
        let dir = self.root.join(path);
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut entries = Vec::new();
        while let Some(entry) = read_dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                entries.push(ChildEntry::Directory(name));
            } else if file_type.is_file() {
                entries.push(ChildEntry::File(name));
            }
        }
        Ok(entries)
    }
}
