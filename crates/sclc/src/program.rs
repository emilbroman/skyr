use std::path::Path;
use std::{collections::HashMap, path::PathBuf};

use thiserror::Error;

use crate::{FileMod, Position, parse_file_mod};

#[derive(Clone)]
pub struct Program {
    deployment: cdb::DeploymentClient,
    files: HashMap<PathBuf, FileMod>,
}

#[derive(Error, Debug)]
pub enum OpenError {
    #[error("failed to load source file: {0}")]
    File(#[from] cdb::FileError),

    #[error("encoding error: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),

    #[error("parse error: {0}")]
    Parse(#[from] peg::error::ParseError<Position>),
}

impl Program {
    pub fn new(deployment: cdb::DeploymentClient) -> Self {
        Self {
            deployment,
            files: HashMap::new(),
        }
    }

    pub async fn open(&mut self, path: impl AsRef<Path>) -> Result<&FileMod, OpenError> {
        let path = path.as_ref().to_path_buf();
        let source = String::from_utf8(self.deployment.read_file(&path).await?)?;
        let file_mod = parse_file_mod(&source)?;
        Ok(self.files.entry(path.clone()).or_insert(file_mod))
    }
}
