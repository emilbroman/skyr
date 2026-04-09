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

/// Overlay source that checks the document cache before delegating to an inner source.
pub struct OverlaySource {
    inner: Box<dyn sclc::SourceRepo>,
    documents: DocumentCache,
    root: PathBuf,
}

impl OverlaySource {
    pub fn new(
        inner: impl sclc::SourceRepo + 'static,
        documents: DocumentCache,
        root: PathBuf,
    ) -> Self {
        Self {
            inner: Box::new(inner),
            documents,
            root,
        }
    }
}

#[async_trait::async_trait]
impl sclc::SourceRepo for OverlaySource {
    fn package_id(&self) -> sclc::PackageId {
        self.inner.package_id()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, sclc::SourceError> {
        // Check if the document is open in the editor
        let absolute = self.root.join(path);
        if let Some(content) = self.documents.get(&absolute) {
            return Ok(Some(content.into_bytes()));
        }
        self.inner.read_file(path).await
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<sclc::ChildEntry>, sclc::SourceError> {
        let mut entries = self.inner.list_children(path).await?;

        // Merge entries from open documents in the editor
        let prefix = self.root.join(path);
        let prefix_str = prefix.to_string_lossy();
        for doc_path in self.documents.paths() {
            let doc_str = doc_path.to_string_lossy();
            let relative = if prefix_str.ends_with('/') || prefix_str.is_empty() {
                doc_str.strip_prefix(prefix_str.as_ref())
            } else {
                doc_str
                    .strip_prefix(prefix_str.as_ref())
                    .and_then(|r| r.strip_prefix('/'))
            };
            if let Some(relative) = relative {
                let entry = if let Some(slash_pos) = relative.find('/') {
                    sclc::ChildEntry::Directory(relative[..slash_pos].to_owned())
                } else {
                    sclc::ChildEntry::File(relative.to_owned())
                };
                if !entries.contains(&entry) {
                    entries.push(entry);
                }
            }
        }

        Ok(entries)
    }
}

/// Result of analyzing a workspace — diagnostics grouped by file URI string.
pub struct AnalysisResult {
    pub diagnostics: HashMap<String, Vec<lsp::Diagnostic>>,
}

/// Run compilation and collect diagnostics.
pub async fn analyze(
    source: impl sclc::SourceRepo + 'static,
    root: &Path,
    package_id: &sclc::PackageId,
) -> AnalysisResult {
    let mut file_diagnostics: HashMap<String, Vec<lsp::Diagnostic>> = HashMap::new();

    match sclc::compile(source).await {
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

/// Load a program with resolved imports (best-effort).
pub async fn load_program(source: impl sclc::SourceRepo + 'static) -> sclc::Program {
    let mut program = sclc::Program::new();
    let package = program.open_package(source).await;
    let _ = package.open("Main.scl").await;
    let _ = program.resolve_imports().await;
    let _ = program.resolve_paths().await;
    program
}

/// Query cursor information at a specific position in a file.
pub fn query_cursor(
    program: &sclc::Program,
    source: &str,
    module_id: &sclc::ModuleId,
    position: sclc::Position,
) -> Arc<Mutex<sclc::CursorInfo>> {
    let cursor = sclc::Cursor::new(position);
    let cursor_info = cursor.info();

    // Parse with cursor
    let diagnosed = sclc::parse_file_mod_with_cursor(source, module_id, Some(cursor.clone()));
    let file_mod = diagnosed.into_inner();

    // Type-check to populate cursor info (declaration, type, references, completions)
    let type_env = sclc::TypeEnv::new()
        .with_module_id(module_id)
        .with_cursor(cursor);
    let unit = sclc::CompilationUnit::from_program(program);
    let checker = sclc::TypeChecker::new(&unit);
    let _ = checker.check_file_mod(&type_env, &file_mod);

    cursor_info
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
