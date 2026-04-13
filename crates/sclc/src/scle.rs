//! SCLE — SCL Expression format.
//!
//! SCLE is a self-contained SCL value format: a sequence of imports, a single
//! type expression declaring the expected type, and a single body expression
//! that evaluates to a value of that type.
//!
//! The grammar is `import_stmt* type_expr? expr?` (see [`crate::ast::ScleMod`]).
//! Both the type expression and the body expression are optional at the
//! grammar level: when only one expression is present, the parser prefers
//! to treat it as the body (synthesizing its type); when the body is
//! missing, a diagnostic is emitted.
//!
//! SCLE modules are first-class citizens of the module graph: any module
//! (including a package's `Main`) may use the `.scle` extension. The loader
//! discovers `.scl` and `.scle` alternatives when resolving imports, the
//! checker validates the body against the declared type expression, and the
//! evaluator evaluates the body to produce the module's value.
//!
//! This module exposes only the parser entry points; loading, checking, and
//! evaluation are handled by the standard pipeline in `loader`, `check`, and
//! `asg_eval`.

use crate::ast::ScleMod;
use crate::{Diagnosed, ModuleId, PackageId, parse_scle};

/// Parse an SCLE source string.
///
/// Returns `Ok(None)` if the source could not be parsed (with diagnostics
/// describing the syntax errors).
pub fn parse_scle_source(source: &str) -> Diagnosed<Option<ScleMod>> {
    parse_scle(source, &scle_module_id())
}

/// A generic [`ModuleId`] used as a placeholder when parsing an SCLE source
/// without a specific module context (e.g. formatter, IDE helpers).
pub fn scle_module_id() -> ModuleId {
    ModuleId::new(PackageId::default(), vec!["Scle".to_string()])
}
