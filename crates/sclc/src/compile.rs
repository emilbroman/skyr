use thiserror::Error;

use crate::{DiagList, Diagnosed, OpenError, Program, ResolveImportError, SourceRepo};

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to open source file: {0}")]
    Open(#[from] OpenError),

    #[error("failed to resolve imports: {0}")]
    ResolveImports(#[from] ResolveImportError),
}

pub async fn compile<S: SourceRepo>(source: S) -> Result<Diagnosed<Program<S>>, CompileError> {
    let mut diags = DiagList::new();
    let mut program = Program::new();
    let package = program.open_package(source).await;
    let _ = package.open("Main.scl").await?;

    program.resolve_imports().await?.unpack(&mut diags);

    Ok(Diagnosed::new(program, diags))
}
