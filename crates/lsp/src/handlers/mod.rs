use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::analysis::{module_id_from_path, uri_to_path};
use crate::document::DocumentCache;

pub mod completion;
pub mod formatting;
pub mod hover;
pub mod lifecycle;
pub mod navigation;

/// Resolved document context from a URI: the file path, source text, and
/// computed module ID.
pub struct DocumentContext {
    pub path: PathBuf,
    pub source: String,
    pub module_id: sclc::ModuleId,
}

/// Resolve a document URI into its path, source content, and module ID.
///
/// Returns `None` when the URI cannot be mapped to a path or the document
/// is not in the cache.
pub fn resolve_document(
    uri: &lsp_types::Uri,
    documents: &DocumentCache,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
) -> Option<DocumentContext> {
    let path = uri_to_path(uri)?;
    let source = documents.get(&path)?;
    let module_id = module_id_from_path(&path, root, package_id);
    Some(DocumentContext {
        path,
        source,
        module_id,
    })
}

/// Lock a `CursorInfo` mutex, recovering gracefully from a poisoned lock
/// instead of panicking.
pub fn lock_cursor_info(
    info: &Arc<Mutex<sclc::CursorInfo>>,
) -> std::sync::MutexGuard<'_, sclc::CursorInfo> {
    info.lock().unwrap_or_else(|poisoned| {
        eprintln!("lsp: cursor info lock was poisoned, recovering");
        poisoned.into_inner()
    })
}
