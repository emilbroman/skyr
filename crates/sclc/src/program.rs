use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

use crate::std::StdSourceRepo;
use crate::{
    ChildEntry, Diag, ModuleId, OpenError, Package, PackageId, PackageLoader, SourceRepo,
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

    pub(crate) async fn ensure_import_package(
        &mut self,
        import_path: &ModuleId,
    ) -> Result<(), ResolveImportError> {
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
        Ok(())
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
