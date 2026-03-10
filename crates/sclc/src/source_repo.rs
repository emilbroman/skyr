use std::{error::Error, path::Path};

use crate::ModuleId;
use crate::std::StdSourceRepo;

#[allow(async_fn_in_trait)]
pub trait SourceRepo {
    type Err: Error + Send + Sync + 'static;

    fn package_id(&self) -> ModuleId;
    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err>;

    fn register_extern(_eval: &mut crate::Eval) {}
}

#[derive(Clone)]
pub enum AnySource<S> {
    User(S),
    Std(StdSourceRepo),
}

#[derive(thiserror::Error, Debug)]
pub enum AnySourceError<E: Error + Send + Sync + 'static> {
    #[error(transparent)]
    User(E),
}

impl<S: SourceRepo> SourceRepo for AnySource<S> {
    type Err = AnySourceError<S::Err>;

    fn package_id(&self) -> ModuleId {
        match self {
            AnySource::User(source) => source.package_id(),
            AnySource::Std(source) => source.package_id(),
        }
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        match self {
            AnySource::User(source) => source.read_file(path).await.map_err(AnySourceError::User),
            AnySource::Std(source) => source.read_file(path).await.map_err(|never| match never {}),
        }
    }

    fn register_extern(eval: &mut crate::Eval) {
        S::register_extern(eval);
        StdSourceRepo::register_extern(eval);
    }
}

impl SourceRepo for cdb::DeploymentClient {
    type Err = cdb::FileError;

    fn package_id(&self) -> ModuleId {
        let repo_qid = self.repo_qid();
        [repo_qid.org.to_string(), repo_qid.repo.to_string()]
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
