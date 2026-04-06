mod ast;
mod checker;
mod compile;
mod cursor;
mod diag;
mod eval;
pub mod fmt;
#[cfg(feature = "fs")]
mod fs_source;
mod lexer;
mod loc;
mod mem_source;
mod module_id;
mod package;
mod package_loader;
mod parser;
mod program;
mod repl;
mod resource;
mod source_repo;
mod std;
pub mod string_escape;
mod ty;
mod value;

pub use ast::*;
pub use checker::*;
pub use compile::*;
pub use cursor::*;
pub use diag::*;
pub use eval::*;
pub use fmt::*;
#[cfg(feature = "fs")]
pub use fs_source::*;
pub use lexer::*;
pub use loc::*;
pub use mem_source::*;
pub use module_id::*;
pub use package::*;
pub use package_loader::*;
pub use parser::*;
pub use program::*;
pub use repl::*;
pub use resource::*;
pub use source_repo::*;
pub use std::*;
pub use ty::*;
pub use value::*;

#[cfg(test)]
mod tests;
