use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::{
    ChildEntry, DiagList, Diagnosed, EvalCtx, EvalError, FileMod, ModuleId, PackageId, Program,
    TrackedValue, Value,
};

/// A fully-resolved compilation unit containing all parsed modules and metadata
/// needed for type checking and evaluation.
///
/// The `CompilationUnit` owns all parsed `FileMod`s in a flat map keyed by
/// `ModuleId`, along with path hashes, directory listings, and extern function
/// implementations. It serves as the immutable context for type checking and
/// evaluation after the resolution phase completes.
#[derive(Clone)]
pub struct CompilationUnit {
    /// Every module in the program, keyed by fully-qualified ModuleId.
    modules: HashMap<ModuleId, FileMod>,

    /// Git object hashes for resolved file paths.
    path_hashes: HashMap<String, gix_hash::ObjectId>,

    /// Cached directory listings for path validation and IDE completions.
    children_cache: HashMap<(PackageId, PathBuf), Vec<ChildEntry>>,

    /// Extern function implementations discovered from loaded packages.
    /// Keyed by fully-qualified name (e.g. "Std/Time.toISO").
    externs: HashMap<String, Value>,

    /// The underlying Program (retained for backward compatibility during
    /// the migration — TypeChecker and Eval still borrow &Program).
    program: Program,
}

/// Error during the resolution phase.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("failed to open module: {0}")]
    Open(#[from] crate::OpenError),

    #[error("I/O error: {0}")]
    Io(#[from] crate::SourceError),

    #[error("failed to resolve imports: {0}")]
    ResolveImport(#[from] crate::ResolveImportError),
}

impl CompilationUnit {
    /// Create a new empty compilation unit.
    pub fn new() -> Self {
        Self {
            modules: HashMap::new(),
            path_hashes: HashMap::new(),
            children_cache: HashMap::new(),
            externs: HashMap::new(),
            program: Program::new(),
        }
    }

    /// Iteratively load, parse, and resolve all modules reachable from the
    /// entry point.
    ///
    /// Uses a load queue: starts with the entry `ModuleId`, loads and parses
    /// it, scans for imports, resolves `Self/` prefixes, and queues any new
    /// modules. Continues until the queue is empty.
    ///
    /// After resolution, collects extern implementations from all loaded
    /// packages and caches path hashes.
    pub async fn resolve(&mut self, entry: &ModuleId) -> Result<Diagnosed<()>, ResolveError> {
        let mut diags = DiagList::new();
        let mut queue = VecDeque::from([entry.clone()]);
        let mut seen_imports = HashSet::new();

        while let Some(module_id) = queue.pop_front() {
            if self.modules.contains_key(&module_id) {
                continue;
            }

            let Some(file_mod) = self.load_module(&module_id, &mut diags).await? else {
                if &module_id == entry {
                    return Ok(Diagnosed::new((), diags));
                }
                continue;
            };

            for statement in &file_mod.statements {
                if let crate::ModStmt::Import(import_stmt) = statement {
                    let raw_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.as_ref().name.clone())
                        .collect();
                    let raw_segments = self.resolve_self_import_segments(raw_segments);
                    if !seen_imports.insert(raw_segments.clone()) {
                        continue;
                    }

                    let import_path =
                        self.split_import_segments(&raw_segments)
                            .unwrap_or_else(|| {
                                ModuleId::new(PackageId::default(), raw_segments.clone())
                            });
                    if self.modules.contains_key(&import_path) {
                        continue;
                    }
                    if self.load_module(&import_path, &mut diags).await?.is_none() {
                        diags.push(invalid_import(module_id.clone(), import_path, import_stmt));
                        continue;
                    }
                    queue.push_back(import_path);
                }
            }

            self.modules.insert(module_id, file_mod);
        }

        self.preload_children_for_completions().await;
        self.resolve_paths().await?;
        self.refresh_externs();

        Ok(Diagnosed::new((), diags))
    }

    async fn load_module(
        &mut self,
        module_id: &ModuleId,
        diags: &mut DiagList,
    ) -> Result<Option<FileMod>, ResolveError> {
        self.program.ensure_import_package(module_id).await?;

        if module_id.path.is_empty() || !module_id.is_safe_path() {
            return Ok(None);
        }

        let module_path = module_id.to_path_buf_with_extension("scl");
        let Some(package) = self.program.packages_mut().get_mut(&module_id.package) else {
            return Ok(None);
        };

        let Some(source) = package.read_module_source(&module_path).await? else {
            return Ok(None);
        };
        let diagnosed = crate::parse_file_mod(&source, module_id);
        Ok(Some(diagnosed.unpack(diags)))
    }

    async fn preload_children_for_completions(&mut self) {
        let mut prefixes_to_load: HashMap<PackageId, HashSet<PathBuf>> = HashMap::new();

        for package_name in self.program.package_names() {
            prefixes_to_load
                .entry(package_name.clone())
                .or_default()
                .insert(PathBuf::new());
        }

        for file_mod in self.modules.values() {
            for statement in &file_mod.statements {
                if let crate::ModStmt::Import(import_stmt) = statement {
                    let raw_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.as_ref().name.clone())
                        .collect();
                    let raw_segments = self.resolve_self_import_segments(raw_segments);
                    if let Some(import_path) = self.split_import_segments(&raw_segments) {
                        let paths = prefixes_to_load
                            .entry(import_path.package.clone())
                            .or_default();
                        let mut prefix = PathBuf::new();
                        for segment in &import_path.path[..import_path.path.len().saturating_sub(1)]
                        {
                            prefix.push(segment);
                            paths.insert(prefix.clone());
                        }
                    }
                }
            }
        }

        for (package_name, paths) in prefixes_to_load {
            if let Some(package) = self.program.packages_mut().get_mut(&package_name) {
                for path in paths {
                    if let Ok(children) = package.list_children(&path).await {
                        self.children_cache
                            .insert((package_name.clone(), path.clone()), children);
                    }
                }
            }
        }
    }

    async fn resolve_paths(&mut self) -> Result<(), ResolveError> {
        let mut dirs_to_preload: HashSet<PathBuf> = HashSet::new();
        let mut resolved_paths: Vec<String> = Vec::new();

        for (module_id, file_mod) in &self.modules {
            let mut collector = crate::ast::CollectPaths::new();
            crate::ast::visit_file_mod(&mut collector, file_mod);
            for path_expr in collector.paths {
                let resolved =
                    path_expr.resolve_with_context(module_id, self.program.self_package_id());
                let rel = resolved.strip_prefix('/').unwrap_or(&resolved);
                let components: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
                let mut prefix = PathBuf::new();
                dirs_to_preload.insert(PathBuf::new());
                for component in &components[..components.len().saturating_sub(1)] {
                    prefix.push(component);
                    dirs_to_preload.insert(prefix.clone());
                }
                resolved_paths.push(resolved);
            }
        }

        self.path_hashes.clear();
        if let Some(pkg_id) = self.program.self_package_id().cloned()
            && let Some(package) = self.program.packages_mut().get_mut(&pkg_id)
        {
            for dir in &dirs_to_preload {
                if let Ok(children) = package.list_children(dir).await {
                    self.children_cache
                        .insert((pkg_id.clone(), dir.clone()), children);
                }
            }

            for resolved in resolved_paths {
                let rel = resolved.strip_prefix('/').unwrap_or(&resolved);
                let repo_path = Path::new(rel);
                if let Ok(Some(hash)) = package.source().path_hash(repo_path).await {
                    self.path_hashes.insert(resolved, hash);
                }
            }
        }

        Ok(())
    }

    fn refresh_externs(&mut self) {
        self.externs.clear();
        for (_package_id, package) in self.program.packages() {
            for (name, value) in package.source().externs() {
                self.externs.insert(name, value);
            }
        }
    }

    /// Look up a module by its ID.
    pub fn module(&self, id: &ModuleId) -> Option<&FileMod> {
        self.modules.get(id)
    }

    /// Iterate over all modules.
    pub fn modules(&self) -> impl Iterator<Item = (&ModuleId, &FileMod)> {
        self.modules.iter()
    }

    /// Look up the Git object hash for a resolved path string.
    pub fn path_hash(&self, resolved: &str) -> Option<&gix_hash::ObjectId> {
        self.path_hashes.get(resolved)
    }

    /// Look up cached children for a package + path.
    pub fn cached_children(&self, package: &PackageId, path: &Path) -> Option<&[ChildEntry]> {
        self.children_cache
            .get(&(package.clone(), path.to_path_buf()))
            .map(Vec::as_slice)
    }

    /// Access extern function implementations.
    pub fn externs(&self) -> &HashMap<String, Value> {
        &self.externs
    }

    /// The package ID of the user's own package (used to resolve `Self/…` imports).
    pub fn self_package_id(&self) -> Option<&PackageId> {
        self.program.self_package_id()
    }

    /// Returns all known package names.
    pub fn package_names(&self) -> impl Iterator<Item = &PackageId> {
        self.program.package_names()
    }

    /// Look up cached children for an import path prefix within a package.
    pub fn cached_children_for_import(
        &self,
        package_name: &PackageId,
        path: &Path,
    ) -> Option<&[ChildEntry]> {
        self.children_cache
            .get(&(package_name.clone(), path.to_path_buf()))
            .map(Vec::as_slice)
    }

    /// Look up cached children for a path within the user's own package.
    pub fn cached_children_for_path(&self, path: &Path) -> Option<&[ChildEntry]> {
        let pkg = self.program.self_package_id()?;
        self.children_cache
            .get(&(pkg.clone(), path.to_path_buf()))
            .map(Vec::as_slice)
    }

    /// Check whether a resolved path exists by consulting cached directory listings.
    /// Returns `None` if any ancestor directory hasn't been cached yet (unknown),
    /// `Some(true)` if the full path is valid, `Some(false)` if invalid.
    pub fn path_exists_cached(&self, resolved: &str) -> Option<bool> {
        let rel = resolved.strip_prefix('/')?;
        let components: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if components.is_empty() {
            return Some(true);
        }

        let mut dir = PathBuf::new();
        for (i, component) in components.iter().enumerate() {
            let is_last = i == components.len() - 1;
            let children = self.cached_children_for_path(&dir)?;

            if is_last {
                let exists = children.iter().any(|entry| entry.name() == *component);
                return Some(exists);
            }

            let is_dir = children
                .iter()
                .any(|entry| matches!(entry, ChildEntry::Directory(name) if name == component));
            if !is_dir {
                let exists_as_non_dir = children
                    .iter()
                    .any(|entry| matches!(entry, ChildEntry::File(name) if name == component));
                if exists_as_non_dir {
                    return Some(false);
                }
                return None;
            }

            dir.push(component);
        }

        None
    }

    /// Type-check all modules in the compilation unit.
    pub fn check_types(&self) -> Result<Diagnosed<()>, crate::TypeCheckError> {
        crate::TypeChecker::new(self).check_program()
    }

    /// Eagerly evaluate every loaded module in the compilation unit.
    pub fn eval(&self, ctx: EvalCtx) -> Result<HashMap<ModuleId, TrackedValue>, EvalError> {
        let eval = crate::Eval::from_ctx(self, ctx);
        let mut module_ids = self.modules.keys().cloned().collect::<Vec<_>>();
        module_ids.sort_by_key(ToString::to_string);

        let mut values = HashMap::new();
        for module_id in module_ids {
            let file_mod = self
                .modules
                .get(&module_id)
                .expect("module ids were collected from the compilation unit");
            let env = crate::EvalEnv::new().with_module_id(&module_id);
            let value = eval.eval_file_mod(&env, file_mod)?;
            values.insert(module_id, value);
        }

        Ok(values)
    }

    /// Set the package loader on the underlying Program.
    pub fn set_package_loader(&mut self, loader: std::sync::Arc<dyn crate::PackageLoader>) {
        self.program.set_package_loader(loader);
    }

    /// Open a user package on the underlying Program.
    pub async fn open_package(
        &mut self,
        source: impl crate::SourceRepo + 'static,
    ) -> &mut crate::Package {
        self.program.open_package(source).await
    }

    /// Replace the user source, clearing all cached state.
    pub fn replace_user_source(&mut self, source: impl crate::SourceRepo + 'static) {
        self.program.replace_user_source(source);
        self.modules.clear();
        self.path_hashes.clear();
        self.children_cache.clear();
        self.externs.clear();
    }

    /// Preload directory listings for a set of repo-relative directory paths.
    pub async fn preload_path_dirs(&mut self, dirs: impl IntoIterator<Item = PathBuf>) {
        self.program.preload_path_dirs(dirs).await;
        // Sync package children caches into our own cache
        if let Some(pkg_id) = self.program.self_package_id().cloned()
            && let Some(package) = self
                .program
                .packages()
                .find(|(id, _)| **id == pkg_id)
                .map(|(_, p)| p)
        {
            for (path, children) in package.children_entries() {
                self.children_cache
                    .insert((pkg_id.clone(), path.clone()), children.clone());
            }
        }
    }

    /// Set the underlying Program directly (used during REPL initialization).
    pub(crate) fn set_program(&mut self, program: Program) {
        self.program = program;
    }

    pub(crate) fn find_imports<'a>(
        &'a self,
        file_mod: &'a FileMod,
    ) -> HashMap<&'a str, (ModuleId, &'a FileMod)> {
        file_mod
            .statements
            .iter()
            .filter_map(|statement| {
                if let crate::ModStmt::Import(import_stmt) = statement {
                    let alias = import_stmt.as_ref().vars.last()?;
                    let raw_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.as_ref().name.clone())
                        .collect();
                    let raw_segments = self.resolve_self_import_segments(raw_segments);
                    let import_path = self.split_import_segments(&raw_segments)?;
                    let destination = self.module(&import_path)?;
                    return Some((alias.as_ref().name.as_str(), (import_path, destination)));
                }
                None
            })
            .collect()
    }

    fn resolve_self_import_segments(&self, segments: Vec<String>) -> Vec<String> {
        if segments.first().map(String::as_str) == Some("Self")
            && let Some(self_pkg) = self.program.self_package_id()
        {
            let mut result: Vec<String> = self_pkg.as_slice().to_vec();
            result.extend(segments[1..].iter().cloned());
            return result;
        }

        segments
    }

    pub(crate) fn split_import_segments(&self, segments: &[String]) -> Option<ModuleId> {
        let package = self
            .program
            .package_names()
            .filter(|package_name| segments.starts_with(package_name.as_slice()))
            .max_by_key(|package_name| package_name.len())
            .cloned()?;
        let pkg_len = package.len();
        Some(ModuleId::new(package, segments[pkg_len..].to_vec()))
    }
}

impl Default for CompilationUnit {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid_import(
    source_module_id: ModuleId,
    import_path: ModuleId,
    import_stmt: &crate::Loc<crate::ImportStmt>,
) -> crate::InvalidImport {
    let vars = &import_stmt.as_ref().vars;
    let path_span = crate::Span::new(
        vars.first()
            .expect("import has at least one segment")
            .span()
            .start(),
        vars.last()
            .expect("import has at least one segment")
            .span()
            .end(),
    );

    crate::InvalidImport {
        source_module_id,
        import_path,
        path_span,
    }
}
