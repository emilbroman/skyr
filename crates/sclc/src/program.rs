use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use thiserror::Error;

use crate::{OpenError, Package};

#[derive(Clone, Default)]
pub struct Program {
    packages: HashMap<Vec<String>, Package>,
}

#[derive(Error, Debug)]
pub enum ResolveImportError {
    #[error("no package found for import path: {import_path:?}")]
    MissingPackage { import_path: Vec<String> },

    #[error("import path has no module suffix after package: {import_path:?}")]
    MissingModulePath { import_path: Vec<String> },

    #[error("failed to open import {import_path:?} from package {package_name:?}: {source}")]
    Open {
        import_path: Vec<String>,
        package_name: Vec<String>,
        module_path: PathBuf,
        #[source]
        source: OpenError,
    },
}

impl Program {
    pub fn new() -> Self {
        Self {
            packages: HashMap::new(),
        }
    }

    pub async fn open_package(&mut self, deployment: cdb::DeploymentClient) -> &mut Package {
        let repository_name = deployment.repository_name();
        let name = vec![
            repository_name.organization.clone(),
            repository_name.repository.clone(),
        ];
        self.packages
            .entry(name)
            .or_insert_with(|| Package::new(deployment))
    }

    pub async fn resolve_imports(&mut self) -> Vec<ResolveImportError> {
        let mut errors = Vec::new();
        let mut seen_import_paths = HashSet::<Vec<String>>::new();

        loop {
            let discovered_import_paths = self
                .packages
                .values()
                .flat_map(|package| package.imports())
                .map(|import_stmt| {
                    import_stmt
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            let pending_import_paths = discovered_import_paths
                .into_iter()
                .filter(|import_path| seen_import_paths.insert(import_path.clone()))
                .collect::<Vec<_>>();

            if pending_import_paths.is_empty() {
                break;
            }

            for import_path in pending_import_paths {
                let Some(package_name) = self.package_name_for_import(&import_path) else {
                    errors.push(ResolveImportError::MissingPackage { import_path });
                    continue;
                };

                let module_segments = &import_path[package_name.len()..];
                if module_segments.is_empty() {
                    errors.push(ResolveImportError::MissingModulePath { import_path });
                    continue;
                }

                let mut module_path = PathBuf::new();
                for segment in module_segments {
                    module_path.push(segment);
                }
                module_path.set_extension("scl");

                let Some(package) = self.packages.get_mut(&package_name) else {
                    errors.push(ResolveImportError::MissingPackage { import_path });
                    continue;
                };

                if let Err(source) = package.open(&module_path).await {
                    errors.push(ResolveImportError::Open {
                        import_path,
                        package_name,
                        module_path,
                        source,
                    });
                }
            }
        }

        errors
    }

    fn package_name_for_import(&self, import_path: &[String]) -> Option<Vec<String>> {
        self.packages
            .keys()
            .filter(|package_name| import_path.starts_with(package_name.as_slice()))
            .max_by_key(|package_name| package_name.len())
            .cloned()
    }
}
