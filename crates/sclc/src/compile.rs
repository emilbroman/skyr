use thiserror::Error;

use crate::{OpenError, Program, ResolveImportError};

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to open source file: {0}")]
    Open(#[from] OpenError),

    #[error("failed to resolve one or more imports")]
    ResolveImports(Vec<ResolveImportError>),
}

pub async fn compile(db: cdb::DeploymentClient) -> Result<Program, CompileError> {
    let mut program = Program::new();
    let package = program.open_package(db).await;
    let _ = package.open("Main.scl").await?;

    let import_errors = program.resolve_imports().await;
    if !import_errors.is_empty() {
        return Err(CompileError::ResolveImports(import_errors));
    }

    Ok(program)
}
