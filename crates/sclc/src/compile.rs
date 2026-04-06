use thiserror::Error;

use std::sync::Arc;

use crate::{
    DiagList, Diagnosed, OpenError, PackageLoader, Program, ResolveImportError, SourceRepo,
    TypeCheckError,
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

pub async fn compile(
    source: impl SourceRepo + 'static,
) -> Result<Diagnosed<Program>, CompileError> {
    compile_inner(source, None).await
}

/// Like [`compile`], but with a [`PackageLoader`] for dynamically
/// discovering packages during import resolution.
pub async fn compile_with_loader(
    source: impl SourceRepo + 'static,
    loader: Arc<dyn PackageLoader>,
) -> Result<Diagnosed<Program>, CompileError> {
    compile_inner(source, Some(loader)).await
}

async fn compile_inner(
    source: impl SourceRepo + 'static,
    loader: Option<Arc<dyn PackageLoader>>,
) -> Result<Diagnosed<Program>, CompileError> {
    let mut diags = DiagList::new();
    let mut program = Program::new();
    if let Some(loader) = loader {
        program.set_package_loader(loader);
    }
    let package = program.open_package(source).await;
    if package.open("Main.scl").await?.unpack(&mut diags).is_none() {
        return Ok(Diagnosed::new(program, diags));
    }

    program.resolve_imports().await?.unpack(&mut diags);
    program.resolve_paths().await?.unpack(&mut diags);
    program.check_types()?.unpack(&mut diags);

    Ok(Diagnosed::new(program, diags))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::ModuleId;

    /// Compiling a program that imports every stdlib module should produce zero diagnostics.
    #[tokio::test]
    async fn stdlib_compiles_without_diagnostics() {
        let stdlib_modules = [
            "Artifact",
            "Container",
            "Crypto",
            "Encoding",
            "List",
            "Num",
            "Option",
            "Random",
            "Time",
        ];

        let main_scl = stdlib_modules
            .iter()
            .map(|m| format!("import Std/{m}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut files = HashMap::new();
        files.insert("Main.scl".to_string(), main_scl.into_bytes());

        let source = crate::MemSourceRepo::new(
            [String::from("Test")].into_iter().collect::<ModuleId>(),
            files,
        );

        let result = super::compile(source)
            .await
            .expect("compilation should not fail");
        let diags: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(
            diags.is_empty(),
            "expected no diagnostics when compiling stdlib, but got: {diags:#?}"
        );
    }
}
