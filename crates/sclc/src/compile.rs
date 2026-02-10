use std::string::FromUtf8Error;

use thiserror::Error;

use crate::ast::Program;

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to load source file: {0}")]
    File(#[from] cdb::FileError),

    #[error("encoding error: {0}")]
    Encoding(#[from] FromUtf8Error),
}

pub async fn compile(db: cdb::DeploymentClient) -> Result<Program, CompileError> {
    let code = String::from_utf8(db.read_file("Main.scl").await?)?;

    Ok(Program { code })
}
