use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_types as lsp;

use crate::convert;
use crate::document::DocumentCache;

/// Overlay source that checks the document cache before delegating to an inner source.
pub struct OverlaySource<S> {
    inner: S,
    documents: DocumentCache,
    root: PathBuf,
}

impl<S> OverlaySource<S> {
    pub fn new(inner: S, documents: DocumentCache, root: PathBuf) -> Self {
        Self {
            inner,
            documents,
            root,
        }
    }
}

impl<S: sclc::SourceRepo> sclc::SourceRepo for OverlaySource<S> {
    type Err = S::Err;

    fn package_id(&self) -> sclc::ModuleId {
        self.inner.package_id()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        // Check if the document is open in the editor
        let absolute = self.root.join(path);
        if let Some(content) = self.documents.get(&absolute) {
            return Ok(Some(content.into_bytes()));
        }
        self.inner.read_file(path).await
    }
}

/// Result of analyzing a workspace — diagnostics grouped by file URI string.
pub struct AnalysisResult {
    pub diagnostics: HashMap<String, Vec<lsp::Diagnostic>>,
}

/// Run compilation and collect diagnostics.
pub async fn analyze<S: sclc::SourceRepo>(source: S, root: &Path) -> AnalysisResult {
    let mut file_diagnostics: HashMap<String, Vec<lsp::Diagnostic>> = HashMap::new();

    match sclc::compile(source).await {
        Ok(diagnosed) => {
            for diag in diagnosed.diags().iter() {
                let (module_id, lsp_diag) = convert::to_lsp_diagnostic(diag);
                let path = module_id_to_path(root, &module_id);
                let uri = path_to_uri_string(&path);
                file_diagnostics.entry(uri).or_default().push(lsp_diag);
            }
        }
        Err(err) => {
            // Compilation hard-failed; report a single diagnostic on Main.scl
            let path = root.join("Main.scl");
            let uri = path_to_uri_string(&path);
            file_diagnostics
                .entry(uri)
                .or_default()
                .push(lsp::Diagnostic {
                    range: lsp::Range::default(),
                    severity: Some(lsp::DiagnosticSeverity::ERROR),
                    source: Some("scl".to_string()),
                    message: err.to_string(),
                    ..Default::default()
                });
        }
    }

    AnalysisResult {
        diagnostics: file_diagnostics,
    }
}

fn module_id_to_path(root: &Path, module_id: &sclc::ModuleId) -> PathBuf {
    let segments = module_id.as_slice();
    if segments.is_empty() {
        return root.join("Main.scl");
    }

    let mut path = root.to_path_buf();
    for (i, segment) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            path.push(format!("{segment}.scl"));
        } else {
            path.push(segment);
        }
    }
    path
}

fn path_to_uri_string(path: &Path) -> String {
    format!("file://{}", path.display())
}

pub fn parse_uri(s: &str) -> lsp::Uri {
    s.parse().unwrap()
}

pub fn uri_to_path(uri: &lsp::Uri) -> Option<PathBuf> {
    uri.as_str().strip_prefix("file://").map(PathBuf::from)
}
