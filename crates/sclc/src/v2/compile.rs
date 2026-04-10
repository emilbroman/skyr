use std::collections::HashMap;
use std::sync::Arc;

use crate::{CompilationUnit, DiagList, Diagnosed, EvalCtx, Value};

use super::{
    Asg, CompositePackageFinder, LoadError, Loader, Package, PackageFinder, StdPackage,
    asg_eval::{AsgEvaluator, EvalResults},
    check::AsgChecker,
};

/// Errors from the v2 compilation pipeline.
#[derive(Debug, thiserror::Error)]
pub enum V2CompileError {
    #[error("load error: {0}")]
    Load(#[from] LoadError),

    #[error("type check error: {0}")]
    TypeCheck(#[from] crate::TypeCheckError),
}

/// Compile using the v2 pipeline: Loader → ASG → AsgChecker.
///
/// Returns the type-checked ASG. Diagnostics (warnings, errors) are attached
/// to the `Diagnosed` wrapper.
pub async fn compile(
    finder: Arc<dyn PackageFinder>,
    entry: &[&str],
) -> Result<Diagnosed<Asg>, V2CompileError> {
    let mut diags = DiagList::new();

    // Build the ASG.
    let mut loader = Loader::new(finder);
    loader.resolve(entry).await?;
    let asg = loader.finish().unpack(&mut diags);

    // Type-check via AsgChecker.
    let _check_results = AsgChecker::new(&asg).check()?.unpack(&mut diags);

    Ok(Diagnosed::new(asg, diags))
}

/// Evaluate using the ASG-driven evaluator.
pub fn eval(asg: &Asg, ctx: EvalCtx) -> Result<EvalResults, crate::EvalError> {
    AsgEvaluator::new(asg, ctx).eval()
}

/// Build a default `PackageFinder` that combines a user package with the
/// standard library.
pub fn build_default_finder(user_package: Arc<dyn Package>) -> Arc<CompositePackageFinder> {
    let std_pkg = Arc::new(StdPackage::new());

    // Wrap each in a PackageFinder.
    Arc::new(CompositePackageFinder::new(vec![
        wrap_as_finder(user_package),
        wrap_as_finder(std_pkg),
    ]))
}

/// Helper to wrap an `Arc<dyn Package>` as an `Arc<dyn PackageFinder>`.
fn wrap_as_finder(pkg: Arc<dyn Package>) -> Arc<dyn PackageFinder> {
    struct PkgFinder(Arc<dyn Package>);

    #[async_trait::async_trait]
    impl PackageFinder for PkgFinder {
        async fn find(&self, raw_id: &[&str]) -> Result<Option<Arc<dyn Package>>, LoadError> {
            let pkg_id = self.0.id();
            let segments = pkg_id.as_slice();
            if raw_id.len() >= segments.len()
                && raw_id[..segments.len()]
                    .iter()
                    .zip(segments.iter())
                    .all(|(a, b)| *a == b.as_str())
            {
                Ok(Some(Arc::clone(&self.0)))
            } else {
                Ok(None)
            }
        }
    }

    Arc::new(PkgFinder(pkg))
}

/// Convert an [`Asg`] into a [`CompilationUnit`] by extracting the modules,
/// externs, and path hashes.
///
/// This is a transitional bridge used by `AsgChecker`, `AsgEvaluator`, and
/// the v2 IDE module while expression-level processing still delegates to the
/// existing `TypeChecker` and `Eval`.
pub fn asg_to_compilation_unit(asg: &Asg) -> CompilationUnit {
    let mut unit = CompilationUnit::new();

    // Register package names so split_import_segments works.
    for pkg_id in asg.packages().keys() {
        unit.register_package_name(pkg_id.clone());
    }

    // Populate modules.
    for module_node in asg.modules() {
        unit.insert_module(module_node.module_id.clone(), module_node.file_mod.clone());
    }

    // Populate externs from all packages.
    let mut externs: HashMap<String, Value> = HashMap::new();
    for pkg in asg.packages().values() {
        pkg.register_externs(&mut externs);
    }
    unit.set_externs(externs);

    unit
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::v2::InMemoryPackage;
    use crate::{ModuleId, PackageId};

    #[tokio::test]
    async fn compile_simple_program() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"export let x = 1".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(msgs.is_empty(), "unexpected diagnostics: {msgs:?}");
    }

    #[tokio::test]
    async fn compile_with_import() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Test/Lib\nexport let x = Lib.foo".to_vec(),
        );
        files.insert(PathBuf::from("Lib.scl"), b"export let foo = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(msgs.is_empty(), "unexpected diagnostics: {msgs:?}");
    }

    #[tokio::test]
    async fn compile_with_stdlib_import() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from("Main.scl"),
            b"import Std/Encoding\nexport let x = Encoding.toJson(1)".to_vec(),
        );

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        let msgs: Vec<String> = result.diags().iter().map(|d| d.to_string()).collect();
        assert!(msgs.is_empty(), "unexpected diagnostics: {msgs:?}");
    }

    #[tokio::test]
    async fn compile_and_eval() {
        let mut files = HashMap::new();
        files.insert(PathBuf::from("Main.scl"), b"export let x = 42".to_vec());

        let user_pkg = Arc::new(InMemoryPackage::new(PackageId::from(["Test"]), files));
        let finder = build_default_finder(user_pkg);

        let result = compile(finder, &["Test", "Main"]).await.unwrap();
        assert!(!result.diags().has_errors());

        let asg = result.into_inner();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = EvalCtx::new(tx, "test");
        let results = eval(&asg, ctx).unwrap();

        let main_id = ModuleId::new(PackageId::from(["Test"]), vec!["Main".to_string()]);
        let main_val = results
            .modules
            .get(&main_id)
            .expect("Main module should have a value");
        assert_eq!(main_val.value.to_string(), "{x: 42}");
    }
}
