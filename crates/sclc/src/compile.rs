use thiserror::Error;

use crate::{
    DiagList, Diagnosed, OpenError, Program, ResolveImportError, SourceRepo, TypeCheckError,
};

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to open source file: {0}")]
    Open(#[from] OpenError),

    #[error("failed to resolve imports: {0}")]
    ResolveImports(#[from] ResolveImportError),

    #[error("failed to type check program: {0}")]
    TypeCheck(#[from] TypeCheckError),
}

pub async fn compile<S: SourceRepo>(source: S) -> Result<Diagnosed<Program<S>>, CompileError> {
    let mut diags = DiagList::new();
    let mut program = Program::new();
    let package = program.open_package(source).await;
    if package.open("Main.scl").await?.unpack(&mut diags).is_none() {
        return Ok(Diagnosed::new(program, diags));
    }

    program.resolve_imports().await?.unpack(&mut diags);
    program.check_types()?.unpack(&mut diags);

    Ok(Diagnosed::new(program, diags))
}
