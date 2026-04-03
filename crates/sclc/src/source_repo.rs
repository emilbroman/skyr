use std::{error::Error, path::Path};

use crate::ModuleId;
use crate::std::StdSourceRepo;

/// An entry returned by [`SourceRepo::list_children`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChildEntry {
    /// A `.scl` module (name without extension).
    Module(String),
    /// A subdirectory that may contain further modules.
    Directory(String),
}

#[allow(async_fn_in_trait)]
pub trait SourceRepo {
    type Err: Error + Send + Sync + 'static;

    fn package_id(&self) -> ModuleId;
    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err>;

    /// List child entries (modules and directories) under the given path
    /// within this source repository.
    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, Self::Err> {
        let _ = path;
        Ok(Vec::new())
    }

    fn register_extern<S2: SourceRepo>(_eval: &mut crate::Eval<'_, S2>) {}
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

    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, Self::Err> {
        match self {
            AnySource::User(source) => source
                .list_children(path)
                .await
                .map_err(AnySourceError::User),
            AnySource::Std(source) => source
                .list_children(path)
                .await
                .map_err(|never| match never {}),
        }
    }

    fn register_extern<S2: SourceRepo>(eval: &mut crate::Eval<'_, S2>) {
        S::register_extern(eval);
        StdSourceRepo::register_extern(eval);
    }
}

#[cfg(feature = "cdb")]
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

    async fn list_children(&self, path: &Path) -> Result<Vec<ChildEntry>, Self::Err> {
        let dir_path = if path.as_os_str().is_empty() {
            None
        } else {
            Some(path)
        };
        match self.read_dir(dir_path).await {
            Ok(tree) => {
                let mut entries = Vec::new();
                for entry in &tree.entries {
                    let name = String::from_utf8_lossy(&entry.filename).into_owned();
                    if entry.mode.is_tree() {
                        entries.push(ChildEntry::Directory(name));
                    } else if entry.mode.is_blob()
                        && let Some(stem) = name.strip_suffix(".scl")
                    {
                        entries.push(ChildEntry::Module(stem.to_owned()));
                    }
                }
                Ok(entries)
            }
            Err(cdb::FileError::NotFound(_)) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }
}
