/// The v2 compiler pipeline.
///
/// This module provides a redesigned Loader → Checker → Evaluator pipeline
/// that replaces the `CompilationUnit`-centric architecture. Shared types
/// (`ast`, `loc`, `diag`, `ty`, `value`, etc.) are imported from the parent
/// crate; only the orchestration layer is new.
mod package;

pub use package::*;
