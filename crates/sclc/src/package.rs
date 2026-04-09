use std::path::{Component, Path};
use std::sync::Arc;
use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

use crate::{
    ChildEntry, DiagList, Diagnosed, FileMod, ImportStmt, Loc, ModStmt, SourceError, SourceRepo,
    parse_file_mod,
};

#[derive(Clone)]
pub struct Package {
    source: Arc<dyn SourceRepo>,
    files: HashMap<PathBuf, FileMod>,
    /// Cached directory listings, keyed by the directory path within the package.
    children_cache: HashMap<PathBuf, Vec<ChildEntry>>,
}

#[derive(Error, Debug)]
pub enum OpenError {
    #[error("module not found: {0}")]
    NotFound(PathBuf),

    #[error("path traversal rejected: {0}")]
    PathTraversal(PathBuf),

    #[error("failed to load source file: {0}")]
    Source(#[from] SourceError),

    #[error("encoding error: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),
}

impl Package {
    pub fn new(source: Arc<dyn SourceRepo>) -> Self {
        Self {
            source,
            files: HashMap::new(),
            children_cache: HashMap::new(),
        }
    }

    pub fn replace_source(&mut self, source: Arc<dyn SourceRepo>) {
        self.source = source;
        self.files.clear();
        self.children_cache.clear();
    }

    pub fn imports(&self) -> impl Iterator<Item = &Loc<ImportStmt>> {
        self.files.values().flat_map(|file_mod| {
            file_mod
                .statements
                .iter()
                .filter_map(|statement| match statement {
                    ModStmt::Import(import_stmt) => Some(import_stmt),
                    ModStmt::Let(_) => None,
                    ModStmt::Export(_) => None,
                    ModStmt::TypeDef(_) => None,
                    ModStmt::ExportTypeDef(_) => None,
                    ModStmt::Expr(_) => None,
                })
        })
    }

    /// Like [`imports`](Self::imports), but also returns the source module ID
    /// for each import statement (the module that contains the import).
    pub fn imports_with_source(&self) -> impl Iterator<Item = (crate::ModuleId, &Loc<ImportStmt>)> {
        let package_id = self.source.package_id();
        self.files.iter().flat_map(move |(path, file_mod)| {
            let module_id = module_id_for_path(&package_id, path);
            file_mod
                .statements
                .iter()
                .filter_map(|statement| match statement {
                    ModStmt::Import(import_stmt) => Some(import_stmt),
                    _ => None,
                })
                .map(move |import_stmt| (module_id.clone(), import_stmt))
        })
    }

    pub fn modules(&self) -> impl Iterator<Item = (&PathBuf, &FileMod)> {
        self.files.iter()
    }

    /// Synchronously look up previously cached children for a path.
    pub fn cached_children(&self, path: &Path) -> Option<&[ChildEntry]> {
        self.children_cache.get(path).map(Vec::as_slice)
    }

    /// Access the underlying source repo.
    pub fn source(&self) -> &dyn SourceRepo {
        &*self.source
    }

    pub fn package_id(&self) -> crate::PackageId {
        self.source.package_id()
    }

    pub async fn open(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Diagnosed<Option<&FileMod>>, OpenError> {
        let path = path.as_ref().to_path_buf();

        // Reject paths that contain traversal components (e.g. ".." or
        // absolute prefixes) to prevent escaping the package directory.
        for component in path.components() {
            match component {
                Component::Normal(_) => {}
                _ => return Err(OpenError::PathTraversal(path)),
            }
        }

        if self.files.contains_key(&path) {
            return Ok(Diagnosed::new(
                Some(
                    self.files
                        .get(&path)
                        .expect("cached file must be present in package map"),
                ),
                DiagList::new(),
            ));
        }

        let source_data = self
            .source
            .read_file(&path)
            .await?
            .ok_or_else(|| OpenError::NotFound(path.clone()))?;
        let source = String::from_utf8(source_data)?;
        let package_id = self.package_id();
        let module_id = module_id_for_path(&package_id, &path);
        let diagnosed = parse_file_mod(&source, &module_id);
        let mut diags = DiagList::new();
        let file_mod = diagnosed.unpack(&mut diags);
        let file_mod = self.files.entry(path.clone()).or_insert(file_mod);
        Ok(Diagnosed::new(Some(file_mod), diags))
    }

    /// List child entries at the given path, caching the result.
    pub async fn list_children(
        &mut self,
        path: impl AsRef<Path>,
    ) -> Result<Vec<ChildEntry>, OpenError> {
        let path = path.as_ref().to_path_buf();
        if let Some(cached) = self.children_cache.get(&path) {
            return Ok(cached.clone());
        }
        let entries = self.source.list_children(&path).await?;
        self.children_cache.insert(path, entries.clone());
        Ok(entries)
    }
}

fn module_id_for_path(package_id: &crate::PackageId, path: &Path) -> crate::ModuleId {
    let mut path_segments = Vec::new();
    if let Some(parent) = path.parent() {
        for segment in parent.components() {
            if let Component::Normal(part) = segment {
                path_segments.push(part.to_string_lossy().into_owned());
            }
        }
    }

    if let Some(stem) = path.file_stem() {
        path_segments.push(stem.to_string_lossy().into_owned());
    }

    crate::ModuleId::new(package_id.clone(), path_segments)
}
