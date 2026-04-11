/// The v2 compiler pipeline: Loader → ASG → AsgChecker → AsgEvaluator.
///
/// Shared types (`ast`, `loc`, `diag`, `ty`, `value`, etc.) are imported
/// from the parent crate; only the orchestration layer is here.
mod asg;
mod asg_eval;
mod cached_package;
mod check;
mod compile;
mod composite_finder;
#[cfg(feature = "fs")]
mod fs_package;
mod ide;
mod loader;
mod mem_package;
mod package;
mod std_package;

pub use asg::*;
pub use asg_eval::*;
pub use cached_package::*;
pub use check::*;
pub use compile::*;
pub use composite_finder::*;
#[cfg(feature = "fs")]
pub use fs_package::*;
pub use ide::*;
pub use loader::*;
pub use mem_package::*;
pub use package::*;
pub use std_package::*;
