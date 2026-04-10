/// The v2 compiler pipeline.
///
/// This module provides a redesigned Loader → Checker → Evaluator pipeline
/// that replaces the `CompilationUnit`-centric architecture. Shared types
/// (`ast`, `loc`, `diag`, `ty`, `value`, etc.) are imported from the parent
/// crate; only the orchestration layer is new.
mod asg;
mod cached_package;
mod composite_finder;
#[cfg(feature = "fs")]
mod fs_package;
mod mem_package;
mod package;
mod std_package;

pub use asg::*;
pub use cached_package::*;
pub use composite_finder::*;
#[cfg(feature = "fs")]
pub use fs_package::*;
pub use mem_package::*;
pub use package::*;
pub use std_package::*;
