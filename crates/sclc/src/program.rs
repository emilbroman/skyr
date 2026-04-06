use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use thiserror::Error;

use crate::{
    AnySource, ChildEntry, Diag, DiagList, Diagnosed, ModuleId, OpenError, Package, SourceRepo, ast,
};
use crate::{TrackedValue, std::StdSourceRepo};

#[derive(Clone, Default)]
pub struct Program<S> {
    packages: HashMap<ModuleId, Package<AnySource<S>>>,
    /// The package ID of the user's own package, used to resolve `Self/…` imports.
    self_package_id: Option<ModuleId>,
    /// Map from resolved path string (e.g. `/data.txt`) to Git object hash.
    path_hashes: HashMap<String, gix_hash::ObjectId>,
}

impl<S> Program<S> {
    pub fn packages(&self) -> impl Iterator<Item = (&ModuleId, &Package<AnySource<S>>)> {
        self.packages.iter()
    }

    /// The package ID of the user's own package (used to resolve `Self/…` imports).
    pub fn self_package_id(&self) -> Option<&ModuleId> {
        self.self_package_id.as_ref()
    }

    pub fn check_types(&self) -> Result<crate::Diagnosed<()>, crate::TypeCheckError>
    where
        S: SourceRepo,
    {
        crate::TypeChecker::new(self).check_program()
    }

    /// Look up cached children for an import path prefix within a package.
    pub fn cached_children_for_import(
        &self,
        package_name: &ModuleId,
        path: &Path,
    ) -> Option<&[ChildEntry]> {
        self.packages.get(package_name)?.cached_children(path)
    }

    /// Returns all known package names.
    pub fn package_names(&self) -> impl Iterator<Item = &ModuleId> {
        self.packages.keys()
    }

    /// Look up the Git object hash for a resolved path string.
    pub fn path_hash(&self, resolved: &str) -> Option<&gix_hash::ObjectId> {
        self.path_hashes.get(resolved)
    }

    /// Look up cached children for a path within the user's own package.
    pub fn cached_children_for_path(&self, path: &Path) -> Option<&[ChildEntry]> {
        let pkg = self.self_package_id.as_ref()?;
        self.packages.get(pkg)?.cached_children(path)
    }

    /// Check whether a resolved path exists by consulting the cached directory
    /// listings. Returns `None` if any ancestor directory hasn't been cached yet
    /// (unknown), `Some(true)` if the full path is valid, `Some(false)` if any
    /// intermediate component is a file (not a directory) or the final component
    /// is missing.
    pub fn path_exists_cached(&self, resolved: &str) -> Option<bool> {
        let resolved_path = Path::new(resolved);

        // Strip the leading `/` to get repo-relative components.
        let rel = resolved.strip_prefix('/')?;
        let components: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if components.is_empty() {
            return Some(true);
        }

        // Walk each component, checking that intermediates are directories
        // and the final component exists.
        let mut dir = PathBuf::new();
        for (i, component) in components.iter().enumerate() {
            let is_last = i == components.len() - 1;
            let children = self.cached_children_for_path(&dir)?;

            if is_last {
                // Final component: just needs to exist (file, module, or directory).
                let exists = children.iter().any(|entry| match entry {
                    ChildEntry::File(name)
                    | ChildEntry::Module(name)
                    | ChildEntry::Directory(name) => name == component,
                });
                return Some(exists);
            }

            // Intermediate component: must be a directory.
            let is_dir = children
                .iter()
                .any(|entry| matches!(entry, ChildEntry::Directory(name) if name == component));
            if !is_dir {
                // It might be a file/module — that means traversing through
                // a non-directory, which is invalid.
                let exists_as_non_dir = children.iter().any(|entry| {
                    matches!(entry, ChildEntry::File(name) | ChildEntry::Module(name) if name == component)
                });
                if exists_as_non_dir {
                    return Some(false);
                }
                // Not cached or doesn't exist at all — unknown.
                return None;
            }

            dir.push(component);
        }

        // Shouldn't reach here, but treat as unknown.
        let _ = resolved_path;
        None
    }
}

#[derive(Error, Debug)]
pub enum ResolveImportError {
    #[error("failed to open import {import_path} from package {package_name}: {source}")]
    Open {
        import_path: ModuleId,
        package_name: ModuleId,
        module_path: PathBuf,
        #[source]
        source: OpenError,
    },
}

#[derive(Error, Debug)]
pub enum EvaluateError {
    #[error("module not loaded: {0}")]
    ModuleNotLoaded(ModuleId),

    #[error("failed to evaluate module {0}: {1}")]
    Eval(ModuleId, #[source] crate::EvalError),
}

#[derive(Error, Debug)]
#[error("module not found: {import_path}")]
pub struct InvalidImport {
    /// The module ID of the file containing the import statement.
    pub source_module_id: ModuleId,
    /// The target import path that could not be resolved.
    pub import_path: ModuleId,
    /// The span covering the import path (first segment start to last segment end).
    pub path_span: crate::Span,
}

impl Diag for InvalidImport {
    fn locate(&self) -> (ModuleId, crate::Span) {
        (self.source_module_id.clone(), self.path_span)
    }
}

impl<S: SourceRepo> Program<S> {
    pub fn new() -> Self {
        let mut packages = HashMap::new();
        let std = StdSourceRepo::new();
        packages.insert(std.package_id(), Package::new(AnySource::Std(std)));
        Self {
            packages,
            self_package_id: None,
            path_hashes: HashMap::new(),
        }
    }

    pub async fn open_package(&mut self, source: S) -> &mut Package<AnySource<S>> {
        let name = SourceRepo::package_id(&source);
        self.self_package_id = Some(name.clone());
        self.packages
            .entry(name)
            .or_insert_with(|| Package::new(AnySource::User(source)))
    }

    pub fn replace_user_source(&mut self, source: S) -> &mut Package<AnySource<S>> {
        let name = SourceRepo::package_id(&source);
        self.self_package_id = Some(name.clone());
        self.path_hashes.clear();
        if self.packages.contains_key(&name) {
            let pkg = self.packages.get_mut(&name).unwrap();
            pkg.replace_source(AnySource::User(source));
            pkg
        } else {
            self.packages
                .entry(name)
                .or_insert_with(|| Package::new(AnySource::User(source)))
        }
    }

    /// If `import_path` starts with `Self`, replace that prefix with the
    /// user package ID so that the rest of the resolution machinery can find
    /// it in the package map.
    fn resolve_self_import(&self, import_path: ModuleId) -> ModuleId {
        if import_path.as_slice().first().map(String::as_str) == Some("Self")
            && let Some(self_id) = &self.self_package_id
        {
            let mut segments: Vec<String> = self_id.as_slice().to_vec();
            segments.extend(import_path.as_slice()[1..].iter().cloned());
            return ModuleId::new(segments);
        }
        import_path
    }

    pub async fn resolve_imports(&mut self) -> Result<Diagnosed<()>, ResolveImportError> {
        let mut diags = DiagList::new();
        let mut seen_import_paths = HashSet::<ModuleId>::new();

        loop {
            let discovered_imports = self
                .packages
                .values()
                .flat_map(|package| package.imports_with_source())
                .map(|(source_module_id, import_stmt)| {
                    let import_path = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.as_ref().name.clone())
                        .collect::<ModuleId>();
                    let import_path = self.resolve_self_import(import_path);
                    (source_module_id, import_path, import_stmt.clone())
                })
                .collect::<Vec<_>>();

            let pending_imports = discovered_imports
                .into_iter()
                .filter(|(_, import_path, _)| seen_import_paths.insert(import_path.clone()))
                .collect::<Vec<_>>();

            if pending_imports.is_empty() {
                break;
            }

            for (source_module_id, import_path, import_stmt) in pending_imports {
                if self
                    .resolve_import(&import_path)
                    .await?
                    .unpack(&mut diags)
                    .is_none()
                {
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
                    diags.push(InvalidImport {
                        source_module_id,
                        import_path,
                        path_span,
                    });
                }
            }
        }

        // Preload children listings for all packages at root level and at each
        // prefix seen in import paths, so the type checker can offer completions
        // synchronously.
        self.preload_children_for_completions().await;

        Ok(Diagnosed::new((), diags))
    }

    /// Preload directory listings for each package so that import completions
    /// can be resolved synchronously during type checking.
    async fn preload_children_for_completions(&mut self) {
        // Collect all import path prefixes to preload.
        let mut prefixes_to_load: HashMap<ModuleId, HashSet<PathBuf>> = HashMap::new();

        // Always preload root for every package.
        for package_name in self.packages.keys() {
            prefixes_to_load
                .entry(package_name.clone())
                .or_default()
                .insert(PathBuf::new());
        }

        // For each import, preload parent directory paths.
        for package in self.packages.values() {
            for import_stmt in package.imports() {
                let import_path = import_stmt
                    .as_ref()
                    .vars
                    .iter()
                    .map(|var| var.as_ref().name.clone())
                    .collect::<ModuleId>();
                let import_path = self.resolve_self_import(import_path);

                if let Some(package_name) = self.package_name_for_import(&import_path)
                    && let Some(module_segments) = import_path.suffix_after(&package_name)
                {
                    let paths = prefixes_to_load.entry(package_name).or_default();
                    // Preload each directory prefix: for Std/Foo/Bar, preload "" and "Foo"
                    let mut prefix = PathBuf::new();
                    for segment in &module_segments[..module_segments.len().saturating_sub(1)] {
                        prefix.push(segment);
                        paths.insert(prefix.clone());
                    }
                }
            }
        }

        // Actually load them.
        for (package_name, paths) in prefixes_to_load {
            if let Some(package) = self.packages.get_mut(&package_name) {
                for path in paths {
                    let _ = package.list_children(&path).await;
                }
            }
        }
    }

    /// Preload directory listings for all path expressions found in loaded
    /// modules. This populates the children cache so the type checker can
    /// validate paths synchronously via [`path_exists_cached`](Program::path_exists_cached).
    ///
    /// Also queries the source repo for Git object hashes of each resolved
    /// path and stores them so the evaluator can build content-addressed
    /// [`PathValue`](crate::PathValue)s.
    pub async fn resolve_paths(&mut self) -> Result<Diagnosed<()>, ResolveImportError> {
        let diags = DiagList::new();

        // Collect all PathExprs from every module, along with their module context.
        let mut collected: Vec<(ModuleId, ast::PathExpr)> = Vec::new();
        for (package_name, package) in &self.packages {
            for (path, file_mod) in package.modules() {
                let module_id = {
                    let mut segments = package_name.as_slice().to_vec();
                    if let Some(parent) = path.parent() {
                        for seg in parent.components() {
                            if let std::path::Component::Normal(part) = seg {
                                segments.push(part.to_string_lossy().into_owned());
                            }
                        }
                    }
                    if let Some(stem) = path.file_stem() {
                        segments.push(stem.to_string_lossy().into_owned());
                    }
                    ModuleId::new(segments)
                };
                let mut collector = ast::CollectPaths::new();
                ast::visit_file_mod(&mut collector, file_mod);
                for path_expr in collector.paths {
                    collected.push((module_id.clone(), path_expr));
                }
            }
        }

        // Resolve each path and determine all ancestor directories to preload.
        let mut dirs_to_preload: HashSet<PathBuf> = HashSet::new();
        let mut resolved_paths: Vec<String> = Vec::new();

        for (module_id, path_expr) in &collected {
            let resolved = path_expr.resolve_with_context(module_id, self.self_package_id.as_ref());

            // Strip leading `/` and preload every ancestor directory.
            let rel = resolved.strip_prefix('/').unwrap_or(&resolved);
            let components: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            let mut prefix = PathBuf::new();
            // Preload root + each intermediate directory (all except the final component).
            dirs_to_preload.insert(PathBuf::new());
            for component in &components[..components.len().saturating_sub(1)] {
                prefix.push(component);
                dirs_to_preload.insert(prefix.clone());
            }

            resolved_paths.push(resolved);
        }

        // Preload directory listings and query path hashes in the user's package.
        if let Some(pkg_id) = &self.self_package_id
            && let Some(package) = self.packages.get_mut(pkg_id)
        {
            for dir in &dirs_to_preload {
                let _ = package.list_children(dir).await;
            }

            // Query hashes for each resolved path.
            for resolved in &resolved_paths {
                let rel = resolved.strip_prefix('/').unwrap_or(resolved);
                let repo_path = Path::new(rel);
                if let Ok(Some(hash)) = package.source().path_hash(repo_path).await {
                    self.path_hashes.insert(resolved.clone(), hash);
                }
            }
        }

        Ok(Diagnosed::new((), diags))
    }

    /// Preload directory listings for a set of repo-relative directory paths.
    /// Useful for the REPL where `resolve_paths` doesn't cover ad-hoc lines.
    pub async fn preload_path_dirs(&mut self, dirs: impl IntoIterator<Item = PathBuf>) {
        if let Some(pkg_id) = &self.self_package_id
            && let Some(package) = self.packages.get_mut(pkg_id)
        {
            for dir in dirs {
                let _ = package.list_children(&dir).await;
            }
        }
    }

    pub async fn resolve_import(
        &mut self,
        import_path: &ModuleId,
    ) -> Result<Diagnosed<Option<&crate::ast::FileMod>>, ResolveImportError> {
        let Some(package_name) = self.package_name_for_import(import_path) else {
            return Ok(Diagnosed::new(None, DiagList::new()));
        };

        let Some(module_segments) = import_path.suffix_after(&package_name) else {
            return Ok(Diagnosed::new(None, DiagList::new()));
        };
        if module_segments.is_empty() {
            return Ok(Diagnosed::new(None, DiagList::new()));
        }

        let module_id_from_segments = module_segments.iter().cloned().collect::<ModuleId>();

        // Reject module paths containing traversal components (e.g. "..")
        // to prevent escaping the package directory.
        if !module_id_from_segments.is_safe_path() {
            return Ok(Diagnosed::new(None, DiagList::new()));
        }

        let module_path = module_id_from_segments.to_path_buf_with_extension("scl");

        let Some(package) = self.packages.get_mut(&package_name) else {
            return Ok(Diagnosed::new(None, DiagList::new()));
        };

        match package.open(&module_path).await {
            Ok(diagnosed) => Ok(diagnosed),
            Err(OpenError::NotFound(_)) => Ok(Diagnosed::new(None, DiagList::new())),
            Err(source) => Err(ResolveImportError::Open {
                import_path: import_path.clone(),
                package_name,
                module_path,
                source,
            }),
        }
    }

    fn package_name_for_import(&self, import_path: &ModuleId) -> Option<ModuleId> {
        self.packages
            .keys()
            .filter(|package_name| import_path.starts_with(package_name))
            .max_by_key(|package_name| package_name.len())
            .cloned()
    }

    pub fn evaluate(
        &self,
        module_id: &ModuleId,
        eval: &crate::Eval<'_, S>,
    ) -> Result<Diagnosed<TrackedValue>, EvaluateError> {
        let Some(file_mod) = self.resolve_import_path(module_id) else {
            return Err(EvaluateError::ModuleNotLoaded(module_id.clone()));
        };

        let env = crate::EvalEnv::new().with_module_id(module_id);
        let result = eval
            .eval_file_mod(&env, file_mod)
            .map_err(|err| EvaluateError::Eval(module_id.clone(), err))?;
        Ok(Diagnosed::new(result, DiagList::new()))
    }

    pub fn find_imports<'a>(
        &'a self,
        file_mod: &'a crate::ast::FileMod,
    ) -> HashMap<&'a str, (ModuleId, &'a crate::ast::FileMod)> {
        file_mod
            .statements
            .iter()
            .filter_map(|statement| {
                if let crate::ast::ModStmt::Import(import_stmt) = statement {
                    let alias = import_stmt.as_ref().vars.last()?;
                    let import_path = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.as_ref().name.clone())
                        .collect::<ModuleId>();
                    let import_path = self.resolve_self_import(import_path);
                    let destination = self.resolve_import_path(&import_path)?;
                    return Some((alias.as_ref().name.as_str(), (import_path, destination)));
                }
                None
            })
            .collect()
    }

    fn resolve_import_path<'a>(
        &'a self,
        import_path: &ModuleId,
    ) -> Option<&'a crate::ast::FileMod> {
        let package_name = self.package_name_for_import(import_path)?;
        let (_, package) = self
            .packages
            .iter()
            .find(|(name, _)| *name == &package_name)?;
        let module_segments = import_path.suffix_after(&package_name)?;
        if module_segments.is_empty() {
            return None;
        }
        let module_id_from_segments = module_segments.iter().cloned().collect::<ModuleId>();
        if !module_id_from_segments.is_safe_path() {
            return None;
        }
        let module_path = module_id_from_segments.to_path_buf_with_extension("scl");
        package
            .modules()
            .find_map(|(path, file_mod)| (path == &module_path).then_some(file_mod))
    }
}
