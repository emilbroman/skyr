use thiserror::Error;

use crate::{OpenError, Program};

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to open source file: {0}")]
    Open(#[from] OpenError),
}

pub async fn compile(db: cdb::DeploymentClient) -> Result<Program, CompileError> {
    let mut program = Program::new(db);
    let _ = program.open("Main.scl").await?;
    Ok(program)
}
