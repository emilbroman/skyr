use std::{error::Error, path::Path};

use crate::ModuleId;

#[allow(async_fn_in_trait)]
pub trait SourceRepo {
    type Err: Error + Send + Sync + 'static;

    fn package_id(&self) -> ModuleId;
    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err>;
}

impl SourceRepo for cdb::DeploymentClient {
    type Err = cdb::FileError;

    fn package_id(&self) -> ModuleId {
        let repository_name = self.repository_name();
        [
            repository_name.organization.clone(),
            repository_name.repository.clone(),
        ]
        .into_iter()
        .collect::<ModuleId>()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        match self.read_file(path).await {
            Ok(data) => Ok(Some(data)),
            Err(cdb::FileError::NotFound(_)) => Ok(None),
            Err(source) => Err(source),
        }
    }
}
