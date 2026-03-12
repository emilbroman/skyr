use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_types::Uri;

use crate::convert::uri_to_path;

#[derive(Debug)]
pub struct Document {
    pub content: String,
    #[allow(dead_code)] // version tracked for future incremental sync support
    pub version: i32,
}

#[derive(Debug, Default)]
pub struct DocumentCache {
    documents: HashMap<PathBuf, Document>,
}

impl DocumentCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, uri: &Uri, content: String, version: i32) {
        if let Some(path) = uri_to_path(uri) {
            self.documents.insert(path, Document { content, version });
        }
    }

    pub fn close(&mut self, uri: &Uri) {
        if let Some(path) = uri_to_path(uri) {
            self.documents.remove(&path);
        }
    }

    pub fn get(&self, path: &Path) -> Option<&str> {
        self.documents.get(path).map(|doc| doc.content.as_str())
    }
}
