use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lsp_types as lsp;

use crate::convert;
use crate::document::DocumentCache;

/// Derive a module ID from a file path.
///
/// When `root` is provided and the path is under it, the module path is
/// computed relative to the workspace root (so `<root>/sub/Foo.scl`
/// becomes `<package_id>/sub/Foo`). This must match the IDs that the
/// compiler uses when building the ASG; otherwise `Self/...` imports in
/// the file won't resolve during cursor queries and types will appear as
/// `Never`.
///
/// Falls back to using the parent directory name as the package and the
/// file stem as the module path when no root is given.
pub fn module_id_from_path(
    path: &Path,
    root: Option<&Path>,
    package_id: &sclc::PackageId,
) -> sclc::ModuleId {
    if let Some(root) = root
        && let Ok(rel) = path.strip_prefix(root)
    {
        let mut segments: Vec<String> = rel
            .parent()
            .into_iter()
            .flat_map(|p| p.components())
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
                _ => None,
            })
            .collect();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Main".to_string());
        segments.push(stem);
        return sclc::ModuleId::new(package_id.clone(), segments);
    }

    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Main".to_string());
    sclc::ModuleId::new(sclc::PackageId::from(["Local"]), vec![stem])
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
            let null_hash = ids::ObjId::null();
            return Ok(Some(Cow::Owned(sclc::PackageEntity::File {
                hash: null_hash,
            })));
        }

        // For directories, merge open documents from the editor.
        let result = self.inner.lookup(path).await?;
        if let Some(Cow::Owned(sclc::PackageEntity::Dir { hash, mut children })) = result {
            let prefix = self.root.join(path);
            let existing_names: HashSet<String> = children.iter().map(|c| c.name.clone()).collect();
            children.extend(dir_children_from_documents(
                &self.documents,
                &prefix,
                &existing_names,
            ));

            return Ok(Some(Cow::Owned(sclc::PackageEntity::Dir {
                hash,
                children,
            })));
        }

        // Check if an open document would make a missing directory appear
        if result.is_none() {
            let prefix = self.root.join(path);
            let null_hash = ids::ObjId::null();
            let children = dir_children_from_documents(&self.documents, &prefix, &HashSet::new());

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

    fn root_path(&self) -> Option<&Path> {
        Some(&self.root)
    }
}

/// Returns `true` if the path has a `.scle` extension.
pub fn is_scle_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "scle")
}

/// Recursively discover all `.scl` and `.scle` files under `root`, skipping
/// hidden directories and common build/dependency directories.
pub async fn discover_workspace_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut rd = match tokio::fs::read_dir(&dir).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        loop {
            let entry = match rd.next_entry().await {
                Ok(Some(e)) => e,
                _ => break,
            };
            let path = entry.path();
            let ft = match entry.file_type().await {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file()
                && let Some(ext) = path.extension()
                && (ext == "scl" || ext == "scle")
            {
                files.push(path);
            }
        }
    }
    files
}

/// Compute the workspace entry list: every `.scl`/`.scle` file under `root`
/// plus any open editor documents (which may be unsaved or outside the
/// on-disk tree we discovered).
async fn workspace_entry_paths(root: &Path, extra_paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = discover_workspace_files(root).await;
    let existing: HashSet<PathBuf> = paths.iter().cloned().collect();
    for p in extra_paths {
        if existing.contains(p) {
            continue;
        }
        // Only include files that live under the workspace root. Files from
        // other packages (e.g. stdlib or cached dependencies opened via
        // goto-definition) should not be treated as workspace entry points.
        if !p.starts_with(root) {
            continue;
        }
        if p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "scl" || e == "scle")
        {
            paths.push(p.clone());
        }
    }
    paths
}

/// Result of analyzing a workspace — diagnostics grouped by file URI string.
pub struct AnalysisResult {
    pub diagnostics: HashMap<String, Vec<lsp::Diagnostic>>,
    /// Absolute paths of files (`.scl` or `.scle`) that were part of the
    /// workspace import graph. Callers can use this to avoid redundantly
    /// analysing the same file through other entry points (e.g. `analyze_scle`).
    pub analyzed_paths: HashSet<PathBuf>,
}

/// Parse, load and type-check a standalone `.scle` file, returning
/// diagnostics.
///
/// The file is loaded via the workspace finder using its real module ID
/// (derived from the path under `root`), so that `Self/...` imports resolve
/// against the workspace package and not a synthetic wrapper.
pub async fn analyze_scle(
    finder: Arc<dyn sclc::PackageFinder>,
    path: &Path,
    root: &Path,
    package_id: &sclc::PackageId,
) -> Vec<lsp::Diagnostic> {
    let module_id = module_id_from_path(path, Some(root), package_id);
    let entry = module_id_to_raw_segments(&module_id);
    let entry_refs: Vec<&str> = entry.iter().map(String::as_str).collect();

    let mut diagnostics = Vec::new();
    match sclc::compile(finder, &entry_refs).await {
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

/// Resolve every workspace `.scl`/`.scle` file (plus any extra open documents)
/// into a single shared `Loader`, then run the type checker. Returns the
/// accumulated ASG and the diagnostic list.
async fn build_workspace_asg(
    finder: Arc<dyn sclc::PackageFinder>,
    root: &Path,
    package_id: &sclc::PackageId,
    extra_paths: &[PathBuf],
) -> (sclc::Asg, sclc::DiagList, HashMap<PathBuf, sclc::LoadError>) {
    let paths = workspace_entry_paths(root, extra_paths).await;

    let mut diags = sclc::DiagList::new();
    let mut loader = sclc::Loader::new(finder);
    let mut load_errors: HashMap<PathBuf, sclc::LoadError> = HashMap::new();
    for path in &paths {
        let module_id = module_id_from_path(path, Some(root), package_id);
        let segments = module_id_to_raw_segments(&module_id);
        let refs: Vec<&str> = segments.iter().map(String::as_str).collect();
        if let Err(err) = loader.resolve(&refs).await {
            load_errors.insert(path.clone(), err);
        }
    }
    let asg = loader.finish().unpack(&mut diags);
    if let Ok(checked) = sclc::AsgChecker::new(&asg).check() {
        let _ = checked.unpack(&mut diags);
    }
    (asg, diags, load_errors)
}

/// Discover and analyze every `.scl`/`.scle` file in the workspace as part of
/// a single shared compilation. Diagnostics are reported regardless of
/// whether a `Main.scl` entry point exists.
pub async fn analyze_workspace(
    finder: Arc<dyn sclc::PackageFinder>,
    root: &Path,
    package_id: &sclc::PackageId,
    extra_paths: &[PathBuf],
) -> AnalysisResult {
    let mut file_diagnostics: HashMap<String, Vec<lsp::Diagnostic>> = HashMap::new();
    let mut analyzed_paths: HashSet<PathBuf> = HashSet::new();

    let (asg, diags, load_errors) =
        build_workspace_asg(finder, root, package_id, extra_paths).await;

    for (path, err) in &load_errors {
        let uri = path_to_uri_string(path);
        file_diagnostics
            .entry(uri)
            .or_default()
            .push(lsp::Diagnostic {
                range: lsp::Range::default(),
                severity: Some(lsp::DiagnosticSeverity::ERROR),
                source: Some(if is_scle_path(path) { "scle" } else { "scl" }.to_string()),
                message: err.to_string(),
                ..Default::default()
            });
    }

    let mut extensions: HashMap<sclc::RawModuleId, &'static str> = HashMap::new();
    for module in asg.modules() {
        let ext = if module.body.is_scle() { "scle" } else { "scl" };
        extensions.insert(module.raw_id.clone(), ext);
        if module.package_id == *package_id {
            analyzed_paths.insert(module_id_to_path(root, &module.module_id, ext));
        }
    }

    let mut seen: HashMap<String, HashSet<(lsp::Range, String, String)>> = HashMap::new();
    // Pre-seed `seen` with any load-error diagnostics already pushed so they
    // don't get duplicated by the diag loop below.
    for (uri, items) in &file_diagnostics {
        for d in items {
            seen.entry(uri.clone()).or_default().insert((
                d.range,
                d.message.clone(),
                severity_label(d.severity).to_string(),
            ));
        }
    }

    for diag in diags.iter() {
        let (module_id, lsp_diag) = convert::to_lsp_diagnostic(diag);
        let raw_id: sclc::RawModuleId = module_id_to_raw_segments(&module_id);
        let ext = extensions.get(&raw_id).copied().unwrap_or("scl");
        let path = module_id_to_path(root, &module_id, ext);
        let uri = path_to_uri_string(&path);
        let key = (
            lsp_diag.range,
            lsp_diag.message.clone(),
            severity_label(lsp_diag.severity).to_string(),
        );
        if seen.entry(uri.clone()).or_default().insert(key) {
            file_diagnostics.entry(uri).or_default().push(lsp_diag);
        }
    }

    AnalysisResult {
        diagnostics: file_diagnostics,
        analyzed_paths,
    }
}

/// Build an ASG covering every workspace file, for cursor queries that need
/// to operate without a `Main.scl` entry point.
pub async fn load_workspace_asg(
    finder: Arc<dyn sclc::PackageFinder>,
    root: &Path,
    package_id: &sclc::PackageId,
    extra_paths: &[PathBuf],
) -> Option<sclc::Asg> {
    let (asg, _diags, _errs) = build_workspace_asg(finder, root, package_id, extra_paths).await;
    Some(asg)
}

/// Query cursor information at a specific position in a file.
///
/// This is the cheap path — suitable for hover, goto-definition, and
/// completion. It only type-checks the cursor's own module, so
/// cross-module references (e.g. for record fields) are not collected.
pub fn query_cursor(
    asg: &sclc::Asg,
    source: &str,
    module_id: &sclc::ModuleId,
    position: sclc::Position,
) -> Arc<Mutex<sclc::CursorInfo>> {
    sclc::cursor_info(asg, module_id, source, position)
}

/// Like [`query_cursor`], but also collects references to the cursor's
/// declaration from every module in the workspace. Use for
/// `textDocument/references`, `textDocument/prepareRename`, and
/// `textDocument/rename` — anywhere the full reference set is needed.
pub fn query_cursor_with_references(
    asg: &sclc::Asg,
    source: &str,
    module_id: &sclc::ModuleId,
    position: sclc::Position,
) -> Arc<Mutex<sclc::CursorInfo>> {
    sclc::cursor_info_with_references(asg, module_id, source, position)
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

/// Flatten a `ModuleId` into the raw segment list used by the loader
/// (package segments followed by module path segments).
fn module_id_to_raw_segments(module_id: &sclc::ModuleId) -> Vec<String> {
    module_id
        .package
        .as_slice()
        .iter()
        .cloned()
        .chain(module_id.path.iter().cloned())
        .collect()
}

/// Convert an LSP diagnostic severity to a short string label for
/// deduplication keys.
fn severity_label(severity: Option<lsp::DiagnosticSeverity>) -> &'static str {
    match severity {
        Some(s) if s == lsp::DiagnosticSeverity::ERROR => "error",
        Some(s) if s == lsp::DiagnosticSeverity::WARNING => "warning",
        Some(s) if s == lsp::DiagnosticSeverity::INFORMATION => "info",
        Some(s) if s == lsp::DiagnosticSeverity::HINT => "hint",
        _ => "unknown",
    }
}

/// Collect directory children from open editor documents whose paths fall
/// under `prefix`. Used by `OverlayPackage::lookup` to merge or synthesize
/// directory listings from unsaved buffers.
fn dir_children_from_documents(
    documents: &DocumentCache,
    prefix: &Path,
    exclude: &HashSet<String>,
) -> Vec<sclc::DirChild> {
    let prefix_str = prefix.to_string_lossy().to_string();
    let null_hash = ids::ObjId::null();
    let mut children = Vec::new();

    for doc_path in documents.paths() {
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
            if !exclude.contains(&name) {
                children.push(sclc::DirChild {
                    name,
                    kind,
                    hash: null_hash,
                });
            }
        }
    }
    children
}

fn module_id_to_path(root: &Path, module_id: &sclc::ModuleId, ext: &str) -> PathBuf {
    // Use the module path directly — it already excludes the package prefix.
    let segments = &module_id.path;

    if segments.is_empty() {
        return root.join(format!("Main.{ext}"));
    }

    let mut path = root.to_path_buf();
    for (i, segment) in segments.iter().enumerate() {
        if i == segments.len() - 1 {
            path.push(format!("{segment}.{ext}"));
        } else {
            path.push(segment);
        }
    }
    path
}

/// Resolve a `RawModuleId` to an LSP URI using the ASG to determine the
/// module's file extension (`.scl` vs `.scle`) and the correct package root
/// for path construction.
///
/// Resolution order for the package root directory:
/// 1. Explicit `package_roots` map (populated by the CLI from cache paths)
/// 2. The `Package::root_path()` from the ASG's stored package
/// 3. The workspace `root` as a fallback for the local package
pub fn raw_module_id_to_uri(
    asg: &sclc::Asg,
    raw_id: &[String],
    root: Option<&Path>,
    package_roots: &HashMap<sclc::PackageId, PathBuf>,
) -> Option<lsp::Uri> {
    let module_node = asg.module(raw_id)?;
    let ext = if module_node.body.is_scle() {
        "scle"
    } else {
        "scl"
    };

    let pkg_root = package_roots
        .get(&module_node.package_id)
        .map(PathBuf::as_path)
        .or_else(|| {
            asg.package(&module_node.package_id)
                .and_then(|pkg| pkg.root_path())
        })
        .or(root)?;

    let path = module_id_to_path(pkg_root, &module_node.module_id, ext);
    Some(parse_uri(&path_to_uri_string(&path)))
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
    fn module_id_from_path_uses_workspace_package_when_root_provided() {
        let root = Path::new("/work/myproj");
        let pkg = sclc::PackageId::from(["MyProj"]);
        let id = module_id_from_path(Path::new("/work/myproj/B.scl"), Some(root), &pkg);
        assert_eq!(id.package, pkg);
        assert_eq!(id.path, vec!["B".to_string()]);

        let id = module_id_from_path(Path::new("/work/myproj/sub/Foo.scl"), Some(root), &pkg);
        assert_eq!(id.package, pkg);
        assert_eq!(id.path, vec!["sub".to_string(), "Foo".to_string()]);
    }

    #[test]
    fn module_id_from_path_falls_back_when_outside_root() {
        let root = Path::new("/work/myproj");
        let pkg = sclc::PackageId::from(["MyProj"]);
        let id = module_id_from_path(Path::new("/elsewhere/B.scl"), Some(root), &pkg);
        // Outside root → fallback uses the constant `Local` package id.
        assert_eq!(id.package, sclc::PackageId::from(["Local"]));
        assert_eq!(id.path, vec!["B".to_string()]);
    }

    #[test]
    fn is_scle_path_recognizes_extension() {
        assert!(is_scle_path(Path::new("/tmp/test.scle")));
        assert!(!is_scle_path(Path::new("/tmp/test.scl")));
        assert!(!is_scle_path(Path::new("/tmp/test.rs")));
    }

    /// Build a finder backed by an in-memory package containing a single
    /// `Foo.scle` file with the given source.
    fn scle_finder(source: &str) -> (Arc<dyn sclc::PackageFinder>, std::path::PathBuf) {
        let mut files = std::collections::HashMap::new();
        files.insert(PathBuf::from("Foo.scle"), source.as_bytes().to_vec());
        let user_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::InMemoryPackage::new(
            sclc::PackageId::from(["Test"]),
            files,
        ));
        let finder = sclc::build_default_finder(user_pkg);
        // The InMemoryPackage uses bare paths, so a "root" of "" combined
        // with a path of "Foo.scle" yields module id Test/Foo.
        (finder, PathBuf::from("Foo.scle"))
    }

    #[tokio::test]
    async fn analyze_scle_valid_source_no_diagnostics() {
        let (finder, path) = scle_finder("Int\n42");
        let pkg = sclc::PackageId::from(["Test"]);
        let diagnostics = analyze_scle(finder, &path, Path::new(""), &pkg).await;
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics for valid SCLE, got: {:?}",
            diagnostics
        );
    }

    #[tokio::test]
    async fn analyze_scle_syntax_error_produces_diagnostics() {
        let (finder, path) = scle_finder("Int");
        let pkg = sclc::PackageId::from(["Test"]);
        let diagnostics = analyze_scle(finder, &path, Path::new(""), &pkg).await;
        assert!(
            !diagnostics.is_empty(),
            "expected diagnostics for incomplete SCLE"
        );
        for diag in &diagnostics {
            assert_eq!(diag.source.as_deref(), Some("scle"));
        }
    }

    #[tokio::test]
    async fn analyze_scle_with_import() {
        let (finder, path) = scle_finder("import Std/List\n[Int]\n[1, 2, 3]");
        let pkg = sclc::PackageId::from(["Test"]);
        let diagnostics = analyze_scle(finder, &path, Path::new(""), &pkg).await;
        assert!(
            diagnostics.is_empty(),
            "expected no diagnostics for valid SCLE with import, got: {:?}",
            diagnostics
        );
    }

    #[tokio::test]
    async fn analyze_scle_self_import_resolves_against_workspace_package() {
        // Regression: previously `analyze_scle` wrapped the source as
        // `__ScleBuffer__/Main`, so `import Self/A` would resolve to
        // `__ScleBuffer__/A` and produce a spurious "module not found" error.
        let mut files = std::collections::HashMap::new();
        files.insert(
            PathBuf::from("Foo.scle"),
            b"import Self/Bar\n\nBar".to_vec(),
        );
        files.insert(PathBuf::from("Bar.scl"), b"export let bar = 1\n".to_vec());
        let user_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::InMemoryPackage::new(
            sclc::PackageId::from(["Test"]),
            files,
        ));
        let finder = sclc::build_default_finder(user_pkg);
        let pkg = sclc::PackageId::from(["Test"]);
        let diagnostics = analyze_scle(finder, Path::new("Foo.scle"), Path::new(""), &pkg).await;
        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(
            !messages.iter().any(|m| m.contains("__ScleBuffer__")),
            "diagnostics should not mention __ScleBuffer__: {:?}",
            messages
        );
    }

    #[tokio::test]
    async fn analyze_scle_type_error_produces_diagnostics() {
        let (finder, path) = scle_finder("Int\n\"not an int\"");
        let pkg = sclc::PackageId::from(["Test"]);
        let diagnostics = analyze_scle(finder, &path, Path::new(""), &pkg).await;
        assert!(
            !diagnostics.is_empty(),
            "expected type error diagnostics for mismatched SCLE body"
        );
        for diag in &diagnostics {
            assert_eq!(diag.source.as_deref(), Some("scle"));
        }
    }

    /// Build a real on-disk workspace with the given files and return the
    /// `tempfile::TempDir` (kept alive for the duration of the test) and the
    /// canonicalized root path.
    fn make_workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir
            .path()
            .canonicalize()
            .expect("canonicalize tempdir root");
        for (rel, contents) in files {
            let path = root.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir -p");
            }
            std::fs::write(&path, contents).expect("write");
        }
        (dir, root)
    }

    #[tokio::test]
    async fn analyze_workspace_reports_diagnostics_without_main() {
        // No Main.scl exists — but A.scl has a type error that should still
        // be surfaced as a diagnostic.
        let (_dir, root) = make_workspace(&[("A.scl", "export let x: Int = \"not an int\"\n")]);
        let pkg = sclc::PackageId::from(["WS"]);
        let fs_pkg = sclc::FsPackage::new(root.clone(), pkg.clone());
        let finder = sclc::build_default_finder(Arc::new(fs_pkg));

        let result = analyze_workspace(finder, &root, &pkg, &[]).await;
        let a_uri = path_to_uri_string(&root.join("A.scl"));
        let diags = result
            .diagnostics
            .get(&a_uri)
            .expect("expected diagnostics for A.scl");
        assert!(
            !diags.is_empty(),
            "expected at least one diagnostic for A.scl: {:?}",
            result.diagnostics
        );
    }

    #[tokio::test]
    async fn analyze_workspace_covers_all_files() {
        // Two unrelated files, each with its own type error: both should
        // show up in the diagnostics map.
        let (_dir, root) = make_workspace(&[
            ("A.scl", "export let x: Int = \"oops\"\n"),
            ("sub/B.scl", "export let y: String = 1\n"),
        ]);
        let pkg = sclc::PackageId::from(["WS"]);
        let fs_pkg = sclc::FsPackage::new(root.clone(), pkg.clone());
        let finder = sclc::build_default_finder(Arc::new(fs_pkg));

        let result = analyze_workspace(finder, &root, &pkg, &[]).await;
        let a_uri = path_to_uri_string(&root.join("A.scl"));
        let b_uri = path_to_uri_string(&root.join("sub").join("B.scl"));
        assert!(
            result.diagnostics.contains_key(&a_uri),
            "expected diagnostics for A.scl: {:?}",
            result.diagnostics.keys().collect::<Vec<_>>()
        );
        assert!(
            result.diagnostics.contains_key(&b_uri),
            "expected diagnostics for sub/B.scl: {:?}",
            result.diagnostics.keys().collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn discover_workspace_files_skips_hidden_and_build_dirs() {
        let (_dir, root) = make_workspace(&[
            ("A.scl", ""),
            ("sub/B.scle", ""),
            (".hidden/C.scl", ""),
            ("target/D.scl", ""),
            ("node_modules/E.scl", ""),
            ("README.md", ""),
        ]);
        let mut found = discover_workspace_files(&root).await;
        found.sort();
        let mut expected = vec![root.join("A.scl"), root.join("sub").join("B.scle")];
        expected.sort();
        assert_eq!(found, expected);
    }
}
