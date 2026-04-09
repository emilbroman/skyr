use std::{collections::HashMap, path::PathBuf, sync::Arc};

use thiserror::Error;

use crate::std::StdSourceRepo;
use crate::{Diag, ModuleId, OpenError, Package, PackageId, PackageLoader, SourceRepo};

#[derive(Clone)]
pub struct Program {
    packages: HashMap<PackageId, Package>,
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

    /// Returns all known package names.
    pub fn package_names(&self) -> impl Iterator<Item = &PackageId> {
        self.packages.keys()
    }

    pub fn new() -> Self {
        let mut packages = HashMap::new();
        let std = StdSourceRepo::new();
        let pkg_id = std.package_id();
        packages.insert(pkg_id, Package::new(Arc::new(std)));
        Self {
            packages,
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
        self.packages
            .entry(name)
            .or_insert_with(|| Package::new(Arc::new(source)))
    }

    pub fn replace_user_source(&mut self, source: impl SourceRepo + 'static) -> &mut Package {
        let name = source.package_id();
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

    /// Preload directory listings for a set of repo-relative directory paths
    /// within the given package.
    /// Useful for the REPL where `resolve_paths` doesn't cover ad-hoc lines.
    pub async fn preload_path_dirs(
        &mut self,
        package_id: &PackageId,
        dirs: impl IntoIterator<Item = PathBuf>,
    ) {
        if let Some(package) = self.packages.get_mut(package_id) {
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
