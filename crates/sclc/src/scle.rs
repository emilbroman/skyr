//! SCLE — SCL Expression format.
//!
//! SCLE is a self-contained SCL value format: a sequence of imports, a single
//! type expression declaring the expected type, and a single body expression
//! that evaluates to a value of that type.
//!
//! The grammar is `import_stmt* type_expr expr` (see [`crate::ast::ScleMod`]).
//!
//! Evaluation reuses the standard SCLC pipeline (Loader → AsgChecker →
//! AsgEvaluator) by synthesising a virtual module that wraps the body in a
//! single `export let __scle_value: T = body` binding under a synthetic
//! package id `__Scle__`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;
use tokio::sync::mpsc;

use crate::ast::{FileMod, LetBind, ModStmt, ScleMod, Var};
use crate::{
    AsgChecker, CompileError, CompositePackageFinder, DiagList, Diagnosed, EvalCtx, EvalError,
    GlobalKey, InMemoryPackage, LoadError, Loader, Loc, ModuleId, Package, PackageFinder,
    PackageId, TrackedValue, TypeCheckError, parse_scle,
};

/// The synthetic package id used to host SCLE evaluations.
const SCLE_PACKAGE: &str = "__Scle__";

/// The synthetic module name within [`SCLE_PACKAGE`].
const SCLE_MODULE: &str = "Main";

/// The name of the synthesised export binding holding the SCLE value.
const SCLE_VALUE_NAME: &str = "__scle_value";

/// Errors from the SCLE pipeline.
#[derive(Debug, Error)]
pub enum ScleError {
    #[error("load error: {0}")]
    Load(#[from] LoadError),

    #[error("type check error: {0}")]
    TypeCheck(#[from] TypeCheckError),

    #[error("evaluation error: {0}")]
    Eval(#[from] EvalError),
}

impl From<CompileError> for ScleError {
    fn from(e: CompileError) -> Self {
        match e {
            CompileError::Load(e) => ScleError::Load(e),
            CompileError::TypeCheck(e) => ScleError::TypeCheck(e),
        }
    }
}

/// Parse an SCLE source string.
///
/// Returns `Ok(None)` if the source could not be parsed (with diagnostics
/// describing the syntax errors).
pub fn parse_scle_source(source: &str) -> Diagnosed<Option<ScleMod>> {
    parse_scle(source, &scle_module_id())
}

/// Evaluate an SCLE source string against the provided [`PackageFinder`].
///
/// The finder is used to resolve any imports declared by the SCLE source.
/// The synthetic `__Scle__` package is automatically prepended so callers do
/// not need to thread it through.
///
/// Returns the evaluated value alongside any diagnostics produced during
/// loading, type-checking or evaluation.
pub async fn evaluate_scle(
    finder: Arc<dyn PackageFinder>,
    source: &str,
) -> Result<Diagnosed<Option<TrackedValue>>, ScleError> {
    let mut diags = DiagList::new();

    // Parse.
    let scle = match parse_scle_source(source).unpack(&mut diags) {
        Some(s) => s,
        None => return Ok(Diagnosed::new(None, diags)),
    };

    // Synthesise a FileMod with the imports and a single exported binding.
    let file_mod = synthesise_file_mod(scle);

    // Compose the user-supplied finder with our synthetic SCLE package.
    let scle_pkg: Arc<dyn Package> = Arc::new(InMemoryPackage::new(
        PackageId::from([SCLE_PACKAGE]),
        HashMap::from([(PathBuf::from("Main.scl"), Vec::new())]),
    ));
    let combined: Arc<dyn PackageFinder> = Arc::new(CompositePackageFinder::new(vec![
        wrap_package_as_finder(Arc::clone(&scle_pkg)),
        finder,
    ]));

    // Inject the synthetic module and resolve transitive imports.
    let mut loader = Loader::new(combined);
    loader
        .resolve_with_entry(&[SCLE_PACKAGE, SCLE_MODULE], scle_pkg, file_mod)
        .await?;
    let asg = loader.finish().unpack(&mut diags);

    // Type-check.
    let _ = AsgChecker::new(&asg).check()?.unpack(&mut diags);
    if diags.has_errors() {
        return Ok(Diagnosed::new(None, diags));
    }

    // Evaluate.
    let (effects_tx, _effects_rx) = mpsc::unbounded_channel();
    let ctx = EvalCtx::new(effects_tx, "scle");
    let (_results, env) = crate::AsgEvaluator::new(&asg, ctx).eval()?;

    let key = GlobalKey::Global(
        vec![SCLE_PACKAGE.to_string(), SCLE_MODULE.to_string()],
        SCLE_VALUE_NAME.to_string(),
    );
    let value = env.get(&key).cloned();

    Ok(Diagnosed::new(value, diags))
}

/// The [`ModuleId`] used by synthetic SCLE modules.
pub fn scle_module_id() -> ModuleId {
    ModuleId::new(
        PackageId::from([SCLE_PACKAGE]),
        vec![SCLE_MODULE.to_string()],
    )
}

fn synthesise_file_mod(scle: ScleMod) -> FileMod {
    let ScleMod {
        imports,
        type_expr,
        body,
    } = scle;

    let mut statements: Vec<ModStmt> = imports.into_iter().map(ModStmt::Import).collect();

    let var_span = type_expr.span();
    let bind = LetBind {
        doc_comment: None,
        var: Loc::new(
            Var {
                name: SCLE_VALUE_NAME.to_string(),
                cursor: None,
            },
            var_span,
        ),
        ty: Some(type_expr),
        expr: Box::new(body),
    };
    statements.push(ModStmt::Export(bind));

    FileMod { statements }
}

/// Wrap an `Arc<dyn Package>` as a `PackageFinder` that matches when the raw
/// id is prefixed by the package id.
fn wrap_package_as_finder(pkg: Arc<dyn Package>) -> Arc<dyn PackageFinder> {
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
