use std::path::{Path, PathBuf};
use std::sync::Arc;

use sclc::{ModuleId, SourceRepo};
use tokio::sync::Mutex;

use crate::document::DocumentCache;

pub struct OverlaySource<S> {
    inner: S,
    documents: Arc<Mutex<DocumentCache>>,
    root: PathBuf,
}

impl<S: SourceRepo> OverlaySource<S> {
    pub fn new(inner: S, documents: Arc<Mutex<DocumentCache>>, root: PathBuf) -> Self {
        Self {
            inner,
            documents,
            root,
        }
    }
}

impl<S: SourceRepo> SourceRepo for OverlaySource<S> {
    type Err = S::Err;

    fn package_id(&self) -> ModuleId {
        self.inner.package_id()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        let abs_path = self.root.join(path);

        let documents = self.documents.lock().await;
        if let Some(content) = documents.get(&abs_path) {
            return Ok(Some(content.as_bytes().to_vec()));
        }
        drop(documents);

        self.inner.read_file(path).await
    }

    fn register_extern(eval: &mut sclc::Eval) {
        S::register_extern(eval);
    }
}
