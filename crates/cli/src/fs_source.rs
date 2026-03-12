use std::path::{Path, PathBuf};

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
}
