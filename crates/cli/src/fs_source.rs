use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct FsSource {
    pub(crate) root: PathBuf,
    pub(crate) package_id: sclc::ModuleId,
}

impl sclc::SourceRepo for FsSource {
    type Err = std::io::Error;

    fn package_id(&self) -> sclc::ModuleId {
        self.package_id.clone()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        match tokio::fs::read(self.root.join(path)).await {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<sclc::ChildEntry>, Self::Err> {
        let dir = self.root.join(path);
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut entries = Vec::new();
        while let Some(entry) = read_dir.next_entry().await? {
            let file_type = entry.file_type().await?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if file_type.is_dir() {
                entries.push(sclc::ChildEntry::Directory(name));
            } else if file_type.is_file() {
                if let Some(stem) = name.strip_suffix(".scl") {
                    entries.push(sclc::ChildEntry::Module(stem.to_owned()));
                } else {
                    entries.push(sclc::ChildEntry::File(name));
                }
            }
        }
        Ok(entries)
    }
}
