use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// In-memory cache of open documents from the editor.
#[derive(Clone, Default)]
pub struct DocumentCache {
    inner: Arc<RwLock<HashMap<PathBuf, DocumentState>>>,
}

struct DocumentState {
    content: String,
    version: i32,
}

impl DocumentCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self, path: PathBuf, content: String, version: i32) {
        let mut docs = self.inner.write().unwrap();
        docs.insert(path, DocumentState { content, version });
    }

    pub fn update(&self, path: &Path, content: String, version: i32) {
        let mut docs = self.inner.write().unwrap();
        if let Some(state) = docs.get_mut(path) {
            state.content = content;
            state.version = version;
        }
    }

    pub fn close(&self, path: &Path) {
        let mut docs = self.inner.write().unwrap();
        docs.remove(path);
    }

    pub fn get(&self, path: &Path) -> Option<String> {
        let docs = self.inner.read().unwrap();
        docs.get(path).map(|state| state.content.clone())
    }

    pub fn version(&self, path: &Path) -> Option<i32> {
        let docs = self.inner.read().unwrap();
        docs.get(path).map(|state| state.version)
    }
}
