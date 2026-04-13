use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lsp_types as lsp;

use crate::convert;
use crate::document::DocumentCache;

/// Derive a module ID from a file path.
///
/// Uses the parent directory name as the package and file stem as the module.
/// Falls back to "Local" if the parent directory name cannot be determined.
pub fn module_id_from_path(path: &Path) -> sclc::ModuleId {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Main".to_string());
    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Local".to_string());
    sclc::ModuleId::new(sclc::PackageId::from([parent_name]), vec![stem])
}

/// A [`sclc::Package`] that overlays editor document contents on top of
/// a filesystem-backed package.
pub struct OverlayPackage {
    inner: sclc::FsPackage,
    documents: DocumentCache,
    root: PathBuf,
}

impl OverlayPackage {
    pub fn new(inner: sclc::FsPackage, documents: DocumentCache, root: PathBuf) -> Self {
        Self {
            inner,
            documents,
            root,
        }
    }
}

#[async_trait::async_trait]
impl sclc::Package for OverlayPackage {
    fn id(&self) -> sclc::PackageId {
        self.inner.id()
    }

    async fn lookup(
        &self,
        path: &Path,
    ) -> Result<Option<Cow<'_, sclc::PackageEntity>>, sclc::LoadError> {
        // If the file is open in the editor, report it as existing.
        let absolute = self.root.join(path);
        if self.documents.get(&absolute).is_some() {
            let null_hash = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);
            return Ok(Some(Cow::Owned(sclc::PackageEntity::File {
                hash: null_hash,
            })));
        }

        // For directories, merge open documents from the editor.
        let result = self.inner.lookup(path).await?;
        if let Some(Cow::Owned(sclc::PackageEntity::Dir { hash, mut children })) = result {
            // Merge entries from open documents in the editor
            let prefix = self.root.join(path);
            let prefix_str = prefix.to_string_lossy().to_string();
            let existing_names: HashSet<String> = children.iter().map(|c| c.name.clone()).collect();
            let null_hash_child = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);

            for doc_path in self.documents.paths() {
                let doc_str = doc_path.to_string_lossy().to_string();
                let relative = if prefix_str.ends_with('/') || prefix_str.is_empty() {
                    doc_str.strip_prefix(&prefix_str).map(|s| s.to_string())
                } else {
                    doc_str
                        .strip_prefix(&prefix_str)
                        .and_then(|r| r.strip_prefix('/'))
                        .map(|s| s.to_string())
                };
                if let Some(relative) = relative {
                    let (name, kind) = if let Some(slash_pos) = relative.find('/') {
                        (relative[..slash_pos].to_owned(), sclc::DirChildKind::Dir)
                    } else {
                        (relative, sclc::DirChildKind::File)
                    };
                    if !existing_names.contains(&name) {
                        children.push(sclc::DirChild {
                            name,
                            kind,
                            hash: null_hash_child,
                        });
                    }
                }
            }

            return Ok(Some(Cow::Owned(sclc::PackageEntity::Dir {
                hash,
                children,
            })));
        }

        // Check if an open document would make a missing directory appear
        if result.is_none() {
            let prefix = self.root.join(path);
            let prefix_str = prefix.to_string_lossy().to_string();
            let null_hash = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);
            let mut children = Vec::new();

            for doc_path in self.documents.paths() {
                let doc_str = doc_path.to_string_lossy().to_string();
                let relative = if prefix_str.ends_with('/') || prefix_str.is_empty() {
                    doc_str.strip_prefix(&prefix_str).map(|s| s.to_string())
                } else {
                    doc_str
                        .strip_prefix(&prefix_str)
                        .and_then(|r| r.strip_prefix('/'))
                        .map(|s| s.to_string())
                };
                if let Some(relative) = relative {
                    let (name, kind) = if let Some(slash_pos) = relative.find('/') {
                        (relative[..slash_pos].to_owned(), sclc::DirChildKind::Dir)
                    } else {
                        (relative, sclc::DirChildKind::File)
                    };
                    children.push(sclc::DirChild {
                        name,
                        kind,
                        hash: null_hash,
                    });
                }
            }

            if !children.is_empty() {
                return Ok(Some(Cow::Owned(sclc::PackageEntity::Dir {
                    hash: null_hash,
                    children,
                })));
            }
        }

        Ok(result.map(|e| Cow::Owned(e.into_owned())))
    }

    async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, sclc::LoadError> {
        let absolute = self.root.join(path);
        if let Some(content) = self.documents.get(&absolute) {
            return Ok(Cow::Owned(content.into_bytes()));
        }
        self.inner.load(path).await
    }
}

/// Returns `true` if the path has a `.scle` extension.
pub fn is_scle_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "scle")
}

/// Result of analyzing a workspace — diagnostics grouped by file URI string.
pub struct AnalysisResult {
    pub diagnostics: HashMap<String, Vec<lsp::Diagnostic>>,
}

/// Run compilation and collect diagnostics.
pub async fn analyze(
    finder: Arc<dyn sclc::PackageFinder>,
    entry: &[&str],
    root: &Path,
    package_id: &sclc::PackageId,
) -> AnalysisResult {
    let mut file_diagnostics: HashMap<String, Vec<lsp::Diagnostic>> = HashMap::new();

    match sclc::compile(finder, entry).await {
        Ok(diagnosed) => {
            // Track seen diagnostics per URI to avoid duplicates.
            let mut seen: HashMap<String, HashSet<(lsp::Range, String, String)>> = HashMap::new();
            for diag in diagnosed.diags().iter() {
                let (module_id, lsp_diag) = convert::to_lsp_diagnostic(diag);
                let path = module_id_to_path(root, &module_id, package_id);
                let uri = path_to_uri_string(&path);
                let severity_str = match lsp_diag.severity {
                    Some(s) if s == lsp::DiagnosticSeverity::ERROR => "error",
                    Some(s) if s == lsp::DiagnosticSeverity::WARNING => "warning",
                    Some(s) if s == lsp::DiagnosticSeverity::INFORMATION => "info",
                    Some(s) if s == lsp::DiagnosticSeverity::HINT => "hint",
                    _ => "unknown",
                };
                let key = (
                    lsp_diag.range,
                    lsp_diag.message.clone(),
                    severity_str.to_string(),
                );
                if seen.entry(uri.clone()).or_default().insert(key) {
                    file_diagnostics.entry(uri).or_default().push(lsp_diag);
                }
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

/// Parse, load and type-check a standalone `.scle` file, returning
/// diagnostics.
///
/// `.scle` files are not part of the module graph, but they can `import` from
/// packages the finder can resolve. We run the full SCLE pipeline up to
/// type-checking (no evaluation) so the editor surfaces the same semantic
/// diagnostics as `.scl` files.
pub async fn analyze_scle(
    finder: Arc<dyn sclc::PackageFinder>,
    source: &str,
    _path: &Path,
) -> Vec<lsp::Diagnostic> {
    let mut diagnostics = Vec::new();
    match sclc::check_scle(finder, source).await {
        Ok(diagnosed) => {
            for diag in diagnosed.diags().iter() {
                let (_module_id, mut lsp_diag) = convert::to_lsp_diagnostic(diag);
                lsp_diag.source = Some("scle".to_string());
                diagnostics.push(lsp_diag);
            }
        }
        Err(err) => {
            diagnostics.push(lsp::Diagnostic {
                range: lsp::Range::default(),
                severity: Some(lsp::DiagnosticSeverity::ERROR),
                source: Some("scle".to_string()),
                message: err.to_string(),
                ..Default::default()
            });
        }
    }
    diagnostics
}

/// Build the ASG for cursor queries.
pub async fn load_asg(finder: Arc<dyn sclc::PackageFinder>, entry: &[&str]) -> Option<sclc::Asg> {
    match sclc::compile(finder, entry).await {
        Ok(diagnosed) => {
            if diagnosed.diags().has_errors() {
                // Still return the ASG — partial results are useful for IDE
            }
            Some(diagnosed.into_inner())
        }
        Err(_) => None,
    }
}

/// Query cursor information at a specific position in a file.
pub fn query_cursor(
    asg: &sclc::Asg,
    source: &str,
    module_id: &sclc::ModuleId,
    position: sclc::Position,
) -> Arc<Mutex<sclc::CursorInfo>> {
    sclc::cursor_info(asg, module_id, source, position)
}

/// Extract document symbols from a parsed file.
pub fn document_symbols(source: &str, module_id: &sclc::ModuleId) -> Vec<lsp::DocumentSymbol> {
    let diagnosed = sclc::parse_file_mod(source, module_id);
    let file_mod = diagnosed.into_inner();

    let mut symbols = Vec::new();
    for stmt in &file_mod.statements {
        match stmt {
            sclc::ModStmt::Let(let_bind) | sclc::ModStmt::Export(let_bind) => {
                let is_export = matches!(stmt, sclc::ModStmt::Export(_));
                #[allow(deprecated)]
                symbols.push(lsp::DocumentSymbol {
                    name: let_bind.var.name.clone(),
                    detail: if is_export {
                        Some("export".to_string())
                    } else {
                        None
                    },
                    kind: symbol_kind_for_expr(&let_bind.expr),
                    tags: None,
                    deprecated: None,
                    range: convert::to_lsp_range(let_bind.expr.span()),
                    selection_range: convert::to_lsp_range(let_bind.var.span()),
                    children: None,
                });
            }
            sclc::ModStmt::TypeDef(type_def) | sclc::ModStmt::ExportTypeDef(type_def) => {
                #[allow(deprecated)]
                symbols.push(lsp::DocumentSymbol {
                    name: type_def.var.name.clone(),
                    detail: Some("type".to_string()),
                    kind: lsp::SymbolKind::STRUCT,
                    tags: None,
                    deprecated: None,
                    range: convert::to_lsp_range(type_def.ty.span()),
                    selection_range: convert::to_lsp_range(type_def.var.span()),
                    children: None,
                });
            }
            sclc::ModStmt::Import(import_stmt) => {
                let name = import_stmt
                    .as_ref()
                    .vars
                    .iter()
                    .map(|v| v.as_ref().name.as_str())
                    .collect::<Vec<_>>()
                    .join("/");
                #[allow(deprecated)]
                symbols.push(lsp::DocumentSymbol {
                    name,
                    detail: Some("import".to_string()),
                    kind: lsp::SymbolKind::MODULE,
                    tags: None,
                    deprecated: None,
                    range: convert::to_lsp_range(import_stmt.span()),
                    selection_range: convert::to_lsp_range(import_stmt.span()),
                    children: None,
                });
            }
            sclc::ModStmt::Expr(_) => {}
        }
    }

    symbols
}

fn symbol_kind_for_expr(expr: &sclc::Loc<sclc::Expr>) -> lsp::SymbolKind {
    match expr.as_ref() {
        sclc::Expr::Fn(_) => lsp::SymbolKind::FUNCTION,
        sclc::Expr::Record(_) => lsp::SymbolKind::STRUCT,
        _ => lsp::SymbolKind::VARIABLE,
    }
}

fn module_id_to_path(
    root: &Path,
    module_id: &sclc::ModuleId,
    _package_id: &sclc::PackageId,
) -> PathBuf {
    // Use the module path directly — it already excludes the package prefix.
    let segments = &module_id.path;

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
    s.parse().unwrap_or_else(|_| {
        // Percent-encode the path portion for URIs with special characters
        if let Some(path) = s.strip_prefix("file://") {
            let encoded: String = path
                .bytes()
                .flat_map(|b| match b {
                    b' ' => b"%20".to_vec(),
                    _ => vec![b],
                })
                .map(|b| b as char)
                .collect();
            format!("file://{encoded}").parse().unwrap_or_else(|_| {
                eprintln!("lsp: failed to parse URI after encoding: {s}");
                "file:///".parse().unwrap()
            })
        } else {
            eprintln!("lsp: non-file URI, falling back to file:///: {s}");
            "file:///".parse().unwrap()
        }
    })
}

pub fn uri_to_path(uri: &lsp::Uri) -> Option<PathBuf> {
    let path = uri.as_str().strip_prefix("file://")?;
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_scle_path_recognizes_extension() {
        assert!(is_scle_path(Path::new("/tmp/test.scle")));
        assert!(!is_scle_path(Path::new("/tmp/test.scl")));
        assert!(!is_scle_path(Path::new("/tmp/test.rs")));
    }

    fn test_finder() -> Arc<dyn sclc::PackageFinder> {
        let user_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::InMemoryPackage::new(
            sclc::PackageId::from(["Test"]),
            std::collections::HashMap::new(),
        ));
        sclc::build_default_finder(user_pkg)
    }

    #[tokio::test]
    async fn analyze_scle_valid_source_no_diagnostics() {
        let path = Path::new("/tmp/test.scle");
        let diagnostics = analyze_scle(test_finder(), "Int\n42", path).await;
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics for valid SCLE, got: {:?}",
            diagnostics
        );
    }

    #[tokio::test]
    async fn analyze_scle_syntax_error_produces_diagnostics() {
        let path = Path::new("/tmp/test.scle");
        let diagnostics = analyze_scle(test_finder(), "Int", path).await;
        assert!(
            !diagnostics.is_empty(),
            "expected diagnostics for incomplete SCLE"
        );
        // Verify the source is "scle"
        for diag in &diagnostics {
            assert_eq!(diag.source.as_deref(), Some("scle"));
        }
    }

    #[tokio::test]
    async fn analyze_scle_with_import() {
        let path = Path::new("/tmp/test.scle");
        let diagnostics =
            analyze_scle(test_finder(), "import Std/List\n[Int]\n[1, 2, 3]", path).await;
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics for valid SCLE with import, got: {:?}",
            diagnostics
        );
    }

    #[tokio::test]
    async fn analyze_scle_type_error_produces_diagnostics() {
        let path = Path::new("/tmp/test.scle");
        let diagnostics = analyze_scle(test_finder(), "Int\n\"not an int\"", path).await;
        assert!(
            !diagnostics.is_empty(),
            "expected type error diagnostics for mismatched SCLE body"
        );
        for diag in &diagnostics {
            assert_eq!(diag.source.as_deref(), Some("scle"));
        }
    }
}
