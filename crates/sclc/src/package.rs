use std::path::{Component, Path};
use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

use crate::{FileMod, ImportStmt, Loc, ModStmt, Position, SourceRepo, parse_file_mod};

#[derive(Clone)]
pub struct Package<S> {
    source: S,
    files: HashMap<PathBuf, FileMod>,
}

#[derive(Error, Debug)]
pub enum OpenError {
    #[error("module not found: {0}")]
    NotFound(PathBuf),

    #[error("failed to load source file: {0}")]
    Source(Box<dyn std::error::Error + Send + Sync>),

    #[error("encoding error: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),

    #[error("parse error: {0}")]
    Parse(#[from] peg::error::ParseError<Position>),
}

impl<S> Package<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            files: HashMap::new(),
        }
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
                    ModStmt::Expr(_) => None,
                })
        })
    }

    pub fn modules(&self) -> impl Iterator<Item = (&PathBuf, &FileMod)> {
        self.files.iter()
    }
}

impl<S: SourceRepo> Package<S> {
    pub fn package_id(&self) -> crate::ModuleId {
        SourceRepo::package_id(&self.source)
    }

    pub async fn open(&mut self, path: impl AsRef<Path>) -> Result<&FileMod, OpenError> {
        let path = path.as_ref().to_path_buf();
        if self.files.contains_key(&path) {
            return Ok(self
                .files
                .get(&path)
                .expect("cached file must be present in package map"));
        }

        let source_data = SourceRepo::read_file(&self.source, &path)
            .await
            .map_err(|err| OpenError::Source(Box::new(err)))?
            .ok_or_else(|| OpenError::NotFound(path.clone()))?;
        let source = String::from_utf8(source_data)?;
        let package_id = self.package_id();
        let module_id = module_id_for_path(&package_id, &path);
        let file_mod = parse_file_mod(&source, &module_id)?.into_inner();
        Ok(self.files.entry(path.clone()).or_insert(file_mod))
    }
}

fn module_id_for_path(package_id: &crate::ModuleId, path: &Path) -> crate::ModuleId {
    let mut segments = package_id.as_slice().to_vec();
    if let Some(parent) = path.parent() {
        for segment in parent.components() {
            if let Component::Normal(part) = segment {
                segments.push(part.to_string_lossy().into_owned());
            }
        }
    }

    if let Some(stem) = path.file_stem() {
        segments.push(stem.to_string_lossy().into_owned());
    }

    segments.into_iter().collect()
}
