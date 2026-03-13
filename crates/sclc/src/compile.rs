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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::path::Path;

    use crate::{ModuleId, SourceRepo};

    /// An in-memory source repository for testing.
    struct MemSourceRepo {
        package_id: ModuleId,
        files: HashMap<String, Vec<u8>>,
    }

    impl SourceRepo for MemSourceRepo {
        type Err = Infallible;

        fn package_id(&self) -> ModuleId {
            self.package_id.clone()
        }

        async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
            let key = path.to_string_lossy().replace('\\', "/");
            Ok(self.files.get(&key).cloned())
        }
    }

    /// Compiling a program that imports every stdlib module should produce zero diagnostics.
    #[tokio::test]
    async fn stdlib_compiles_without_diagnostics() {
        let mut files = HashMap::new();
        files.insert(
            "Main.scl".to_string(),
            b"import Std/Option\nimport Std/List\nimport Std/Num\nimport Std/Artifact\nimport Std/Container\nimport Std/Crypto\nimport Std/Encoding\nimport Std/Random\n".to_vec(),
        );

        let source = MemSourceRepo {
            package_id: [String::from("Test")].into_iter().collect(),
            files,
        };

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
