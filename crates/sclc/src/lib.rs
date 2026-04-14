mod asg;
mod asg_eval;
mod ast;
mod cached_package;
mod check;
mod checker;
mod compile;
mod composite_finder;
#[cfg(feature = "cdb")]
mod cross_repo;
mod cursor;
mod dep_graph;
mod diag;
mod eval;
pub mod fmt;
#[cfg(feature = "fs")]
mod fs_package;
mod ide;
mod lexer;
mod loader;
mod loc;
mod manifest;
mod mem_package;
mod module_id;
mod package;
mod parser;
mod repl;
mod resource;
mod scle;
mod std;
mod std_package;
pub mod string_escape;
mod ty;
mod value;

pub use asg::*;
pub use asg_eval::*;
pub use ast::*;
pub use cached_package::*;
pub use check::*;
pub use checker::*;
pub use compile::*;
pub use composite_finder::*;
#[cfg(feature = "cdb")]
pub use cross_repo::*;
pub use cursor::*;
pub use diag::*;
pub use eval::*;
pub use fmt::*;
#[cfg(feature = "fs")]
pub use fs_package::*;
pub use ide::*;
pub use lexer::*;
pub use loader::*;
pub use loc::*;
pub use manifest::*;
pub use mem_package::*;
pub use module_id::*;
pub use package::*;
pub use parser::*;
pub use repl::*;
pub use resource::*;
pub use scle::*;
pub use std::*;
pub use std_package::*;
pub use ty::*;
pub use value::*;

/// The sclc crate version, used as the stdlib cache key.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A placeholder [`ids::DeploymentQid`] suitable for tests and other contexts
/// that need to construct an [`EvalCtx`] without a real deployment behind it
/// (e.g. the REPL, the wasm playground, or in-memory unit tests).
pub fn placeholder_deployment_qid() -> ids::DeploymentQid {
    "placeholder/placeholder::main@0000000000000000000000000000000000000000"
        .parse()
        .expect("placeholder deployment QID is well-formed")
}

#[cfg(test)]
mod tests;
