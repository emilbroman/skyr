use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Maximum number of documents that can be cached simultaneously.
const MAX_DOCUMENTS: usize = 512;

/// Maximum size in bytes for a single document.
const MAX_DOCUMENT_SIZE: usize = 10 * 1024 * 1024; // 10 MiB

/// In-memory cache of open documents from the editor.
#[derive(Clone, Default)]
pub struct DocumentCache {
    inner: Arc<RwLock<HashMap<PathBuf, DocumentState>>>,
}

struct DocumentState {
    content: String,
    version: i32,
}

/// Acquire a write lock, recovering from a poisoned lock.
fn write_lock(
    lock: &RwLock<HashMap<PathBuf, DocumentState>>,
) -> std::sync::RwLockWriteGuard<'_, HashMap<PathBuf, DocumentState>> {
    lock.write().unwrap_or_else(|poisoned| {
        eprintln!("lsp: DocumentCache write lock was poisoned, recovering");
        poisoned.into_inner()
    })
}

/// Acquire a read lock, recovering from a poisoned lock.
fn read_lock(
    lock: &RwLock<HashMap<PathBuf, DocumentState>>,
) -> std::sync::RwLockReadGuard<'_, HashMap<PathBuf, DocumentState>> {
    lock.read().unwrap_or_else(|poisoned| {
        eprintln!("lsp: DocumentCache read lock was poisoned, recovering");
        poisoned.into_inner()
    })
}

impl DocumentCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self, path: PathBuf, content: String, version: i32) {
        if content.len() > MAX_DOCUMENT_SIZE {
            eprintln!(
                "lsp: refusing to cache document {:?} ({} bytes exceeds {} limit)",
                path,
                content.len(),
                MAX_DOCUMENT_SIZE
            );
            return;
        }
        let mut docs = write_lock(&self.inner);
        if docs.len() >= MAX_DOCUMENTS && !docs.contains_key(&path) {
            eprintln!(
                "lsp: document cache full ({MAX_DOCUMENTS} documents), refusing to cache {:?}",
                path
            );
            return;
        }
        docs.insert(path, DocumentState { content, version });
    }

    pub fn update(&self, path: &Path, content: String, version: i32) {
        if content.len() > MAX_DOCUMENT_SIZE {
            eprintln!(
                "lsp: refusing to update document {:?} ({} bytes exceeds {} limit)",
                path,
                content.len(),
                MAX_DOCUMENT_SIZE
            );
            return;
        }
        let mut docs = write_lock(&self.inner);
        if let Some(state) = docs.get_mut(path) {
            state.content = content;
            state.version = version;
        }
    }

    pub fn close(&self, path: &Path) {
        let mut docs = write_lock(&self.inner);
        docs.remove(path);
    }

    pub fn get(&self, path: &Path) -> Option<String> {
        let docs = read_lock(&self.inner);
        docs.get(path).map(|state| state.content.clone())
    }

    pub fn version(&self, path: &Path) -> Option<i32> {
        let docs = read_lock(&self.inner);
        docs.get(path).map(|state| state.version)
    }
}
