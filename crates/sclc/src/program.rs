use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

use crate::std::StdSourceRepo;
use crate::{
    ChildEntry, Diag, DiagList, Diagnosed, ModuleId, OpenError, Package, PackageId, PackageLoader,
    SourceRepo, ast,
};

#[derive(Clone)]
pub struct Program {
    packages: HashMap<PackageId, Package>,
    /// The package ID of the user's own package, used to resolve `Self/…` imports.
    self_package_id: Option<PackageId>,
    /// Map from resolved path string (e.g. `/data.txt`) to Git object hash.
    path_hashes: HashMap<String, gix_hash::ObjectId>,
    /// Optional loader for dynamically discovering packages during import resolution.
    package_loader: Option<Arc<dyn PackageLoader>>,
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}

impl Program {
    pub fn packages(&self) -> impl Iterator<Item = (&PackageId, &Package)> {
        self.packages.iter()
    }

    /// Mutable access to the packages map.
    pub fn packages_mut(&mut self) -> &mut HashMap<PackageId, Package> {
        &mut self.packages
    }

    /// The package ID of the user's own package (used to resolve `Self/…` imports).
    pub fn self_package_id(&self) -> Option<&PackageId> {
        self.self_package_id.as_ref()
    }

    /// Look up cached children for an import path prefix within a package.
    pub fn cached_children_for_import(
        &self,
        package_name: &PackageId,
        path: &Path,
    ) -> Option<&[ChildEntry]> {
        self.packages.get(package_name)?.cached_children(path)
    }

    /// Returns all known package names.
    pub fn package_names(&self) -> impl Iterator<Item = &PackageId> {
        self.packages.keys()
    }

    /// Look up the Git object hash for a resolved path string.
    pub fn path_hash(&self, resolved: &str) -> Option<&gix_hash::ObjectId> {
        self.path_hashes.get(resolved)
    }

    /// Access all path hashes.
    pub fn path_hashes(&self) -> &HashMap<String, gix_hash::ObjectId> {
        &self.path_hashes
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
                // Final component: just needs to exist (file or directory).
                let exists = children.iter().any(|entry| entry.name() == *component);
                return Some(exists);
            }

            // Intermediate component: must be a directory.
            let is_dir = children
                .iter()
                .any(|entry| matches!(entry, ChildEntry::Directory(name) if name == component));
            if !is_dir {
                // It might be a file — that means traversing through
                // a non-directory, which is invalid.
                let exists_as_non_dir = children
                    .iter()
                    .any(|entry| matches!(entry, ChildEntry::File(name) if name == component));
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

    pub fn new() -> Self {
        let mut packages = HashMap::new();
        let std = StdSourceRepo::new();
        let pkg_id = std.package_id();
        packages.insert(pkg_id, Package::new(Arc::new(std)));
        Self {
            packages,
            self_package_id: None,
            path_hashes: HashMap::new(),
            package_loader: None,
        }
    }

    /// Set the package loader used to dynamically discover packages during
    /// import resolution. See [`PackageLoader`] for details.
    pub fn set_package_loader(&mut self, loader: Arc<dyn PackageLoader>) {
        self.package_loader = Some(loader);
    }

    pub async fn open_package(&mut self, source: impl SourceRepo + 'static) -> &mut Package {
        let name = source.package_id();
        self.self_package_id = Some(name.clone());
        self.packages
            .entry(name)
            .or_insert_with(|| Package::new(Arc::new(source)))
    }

    pub fn replace_user_source(&mut self, source: impl SourceRepo + 'static) -> &mut Package {
        let name = source.package_id();
        self.self_package_id = Some(name.clone());
        self.path_hashes.clear();
        if self.packages.contains_key(&name) {
            let pkg = self.packages.get_mut(&name).unwrap();
            pkg.replace_source(Arc::new(source));
            pkg
        } else {
            self.packages
                .entry(name)
                .or_insert_with(|| Package::new(Arc::new(source)))
        }
    }

    /// If `segments` starts with `Self`, replace that prefix with the
    /// user package ID segments so that the rest of the resolution machinery
    /// can find it in the package map.
    fn resolve_self_import_segments(&self, segments: Vec<String>) -> Vec<String> {
        if segments.first().map(String::as_str) == Some("Self")
            && let Some(self_pkg) = &self.self_package_id
        {
            let mut result: Vec<String> = self_pkg.as_slice().to_vec();
            result.extend(segments[1..].iter().cloned());
            return result;
        }
        segments
    }

    /// Split raw import segments into a `ModuleId` using known packages.
    fn split_import_segments(&self, segments: &[String]) -> Option<ModuleId> {
        let package = self.package_name_for_import(segments)?;
        let pkg_len = package.len();
        let path = segments[pkg_len..].to_vec();
        Some(ModuleId::new(package, path))
    }

    pub async fn resolve_imports(&mut self) -> Result<Diagnosed<()>, ResolveImportError> {
        let mut diags = DiagList::new();
        let mut seen_import_segments = HashSet::<Vec<String>>::new();

        loop {
            let discovered_imports = self
                .packages
                .values()
                .flat_map(|package| package.imports_with_source())
                .map(|(source_module_id, import_stmt)| {
                    let raw_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.as_ref().name.clone())
                        .collect();
                    let raw_segments = self.resolve_self_import_segments(raw_segments);
                    (source_module_id, raw_segments, import_stmt.clone())
                })
                .collect::<Vec<_>>();

            let pending_imports = discovered_imports
                .into_iter()
                .filter(|(_, segments, _)| seen_import_segments.insert(segments.clone()))
                .collect::<Vec<_>>();

            if pending_imports.is_empty() {
                break;
            }

            for (source_module_id, raw_segments, import_stmt) in pending_imports {
                let import_path = self
                    .split_import_segments(&raw_segments)
                    .unwrap_or_else(|| {
                        // Best-effort: put everything in path with empty package
                        ModuleId::new(PackageId::default(), raw_segments.clone())
                    });
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
        let mut prefixes_to_load: HashMap<PackageId, HashSet<PathBuf>> = HashMap::new();

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
                    // Preload each directory prefix: for Std/Foo/Bar, preload "" and "Foo"
                    let mut prefix = PathBuf::new();
                    let module_segments = &import_path.path;
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
        for (package_id, package) in &self.packages {
            for (path, file_mod) in package.modules() {
                let module_id = {
                    let mut path_segments = Vec::new();
                    if let Some(parent) = path.parent() {
                        for seg in parent.components() {
                            if let std::path::Component::Normal(part) = seg {
                                path_segments.push(part.to_string_lossy().into_owned());
                            }
                        }
                    }
                    if let Some(stem) = path.file_stem() {
                        path_segments.push(stem.to_string_lossy().into_owned());
                    }
                    ModuleId::new(package_id.clone(), path_segments)
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
        let all_segments = import_path.all_segments();

        // If no loaded package matches, try the package loader.
        if self.package_name_for_import(&all_segments).is_none()
            && let Some(loader) = self.package_loader.clone()
        {
            match loader.load_package(import_path).await {
                Ok(Some(source)) => {
                    let pkg_id: PackageId = source.package_id();
                    self.packages
                        .entry(pkg_id)
                        .or_insert_with(|| Package::new(source));
                }
                Ok(None) => {}
                Err(source) => {
                    return Err(ResolveImportError::Loader {
                        import_path: import_path.clone(),
                        source,
                    });
                }
            }
        }

        // Use the module path from the ModuleId directly
        if import_path.path.is_empty() {
            return Ok(Diagnosed::new(None, DiagList::new()));
        }

        let package_name = &import_path.package;

        // Reject module paths containing traversal components (e.g. "..")
        // to prevent escaping the package directory.
        if !import_path.is_safe_path() {
            return Ok(Diagnosed::new(None, DiagList::new()));
        }

        let module_path = import_path.to_path_buf_with_extension("scl");

        let Some(package) = self.packages.get_mut(package_name) else {
            return Ok(Diagnosed::new(None, DiagList::new()));
        };

        match package.open(&module_path).await {
            Ok(diagnosed) => Ok(diagnosed),
            Err(OpenError::NotFound(_)) => Ok(Diagnosed::new(None, DiagList::new())),
            Err(source) => Err(ResolveImportError::Open {
                import_path: import_path.clone(),
                package_name: package_name.clone(),
                module_path,
                source,
            }),
        }
    }

    fn package_name_for_import(&self, segments: &[String]) -> Option<PackageId> {
        self.packages
            .keys()
            .filter(|package_name| segments.starts_with(package_name.as_slice()))
            .max_by_key(|package_name| package_name.len())
            .cloned()
    }
}

#[derive(Error, Debug)]
pub enum ResolveImportError {
    #[error("failed to open import {import_path} from package {package_name}: {source}")]
    Open {
        import_path: ModuleId,
        package_name: PackageId,
        module_path: PathBuf,
        #[source]
        source: OpenError,
    },

    #[error("package loader failed for {import_path}: {source}")]
    Loader {
        import_path: ModuleId,
        #[source]
        source: crate::SourceError,
    },
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
