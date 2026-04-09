use thiserror::Error;

use std::sync::Arc;

use crate::{
    CompilationUnit, DiagList, Diagnosed, ModuleId, OpenError, PackageId, PackageLoader,
    ResolveError, ResolveImportError, SourceRepo, TypeCheckError,
};

#[derive(Error, Debug)]
pub enum CompileError {
    #[error("failed to open source file: {0}")]
    Open(#[from] OpenError),

    #[error("failed to resolve imports: {0}")]
    ResolveImports(#[from] ResolveImportError),

    #[error("failed to type check program: {0}")]
    TypeCheck(#[from] TypeCheckError),

    #[error("failed to resolve: {0}")]
    Resolve(#[from] ResolveError),
}

pub async fn compile(
    source: impl SourceRepo + 'static,
) -> Result<Diagnosed<CompilationUnit>, CompileError> {
    compile_inner(source, None).await
}

/// Like [`compile`], but with a [`PackageLoader`] for dynamically
/// discovering packages during import resolution.
pub async fn compile_with_loader(
    source: impl SourceRepo + 'static,
    loader: Arc<dyn PackageLoader>,
) -> Result<Diagnosed<CompilationUnit>, CompileError> {
    compile_inner(source, Some(loader)).await
}

async fn compile_inner(
    source: impl SourceRepo + 'static,
    loader: Option<Arc<dyn PackageLoader>>,
) -> Result<Diagnosed<CompilationUnit>, CompileError> {
    let mut diags = DiagList::new();

    let package_id: PackageId = source.package_id();
    let mut unit = CompilationUnit::new();

    if let Some(loader) = loader {
        unit.set_package_loader(loader);
    }
    unit.open_package(source).await;

    let entry = ModuleId::new(package_id, vec!["Main".to_string()]);
    unit.resolve(&entry).await?.unpack(&mut diags);

    unit.check_types()?.unpack(&mut diags);

    Ok(Diagnosed::new(unit, diags))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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
            [String::from("Test")]
                .into_iter()
                .collect::<crate::PackageId>(),
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
