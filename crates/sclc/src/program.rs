use std::{collections::HashMap, path::PathBuf, sync::Arc};

use thiserror::Error;

use crate::std::StdSourceRepo;
use crate::{Diag, ModuleId, OpenError, Package, PackageId, SourceRepo};

#[derive(Clone)]
pub struct Program {
    packages: HashMap<PackageId, Package>,
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
        Self { packages }
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
        _import_path: &ModuleId,
    ) -> Result<(), ResolveImportError> {
        Ok(())
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
