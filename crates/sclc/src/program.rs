use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use thiserror::Error;

use crate::{Diag, DiagList, Diagnosed, ImportStmt, Loc, ModuleId, OpenError, Package, SourceRepo};

#[derive(Clone, Default)]
pub struct Program<S> {
    packages: HashMap<ModuleId, Package<S>>,
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
    #[error("module id has no module path after package: {0}")]
    MissingModulePath(ModuleId),

    #[error("module not loaded: {0}")]
    ModuleNotLoaded(ModuleId),

    #[error("failed to open module {0}: {1}")]
    Open(ModuleId, #[source] OpenError),

    #[error("failed to evaluate module {0}: {1}")]
    Eval(ModuleId, #[source] crate::EvalError),
}

#[derive(Error, Debug)]
#[error("invalid import path: {module_id}")]
pub struct InvalidImport {
    pub module_id: ModuleId,
    pub import: Loc<ImportStmt>,
}

impl Diag for InvalidImport {
    fn locate(&self) -> (ModuleId, crate::Span) {
        (self.module_id.clone(), self.import.span())
    }
}

impl<S: SourceRepo> Program<S> {
    pub fn new() -> Self {
        Self {
            packages: HashMap::new(),
        }
    }

    pub async fn open_package(&mut self, source: S) -> &mut Package<S> {
        let name = SourceRepo::package_id(&source);
        self.packages
            .entry(name)
            .or_insert_with(|| Package::new(source))
    }

    pub async fn resolve_imports(&mut self) -> Result<Diagnosed<()>, ResolveImportError> {
        let mut diags = DiagList::new();
        let mut seen_import_paths = HashSet::<ModuleId>::new();

        loop {
            let discovered_imports = self
                .packages
                .values()
                .flat_map(|package| package.imports())
                .map(|import_stmt| {
                    let import_path = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect::<ModuleId>();
                    (import_path, import_stmt.clone())
                })
                .collect::<Vec<_>>();

            let pending_imports = discovered_imports
                .into_iter()
                .filter(|(import_path, _)| seen_import_paths.insert(import_path.clone()))
                .collect::<Vec<_>>();

            if pending_imports.is_empty() {
                break;
            }

            for (import_path, import_stmt) in pending_imports {
                let Some(package_name) = self.package_name_for_import(&import_path) else {
                    diags.push(InvalidImport {
                        module_id: import_path,
                        import: import_stmt,
                    });
                    continue;
                };

                let Some(module_segments) = import_path.suffix_after(&package_name) else {
                    diags.push(InvalidImport {
                        module_id: import_path,
                        import: import_stmt,
                    });
                    continue;
                };
                if module_segments.is_empty() {
                    diags.push(InvalidImport {
                        module_id: import_path,
                        import: import_stmt,
                    });
                    continue;
                }

                let module_path = module_segments
                    .iter()
                    .cloned()
                    .collect::<ModuleId>()
                    .to_path_buf_with_extension("scl");

                let Some(package) = self.packages.get_mut(&package_name) else {
                    diags.push(InvalidImport {
                        module_id: import_path,
                        import: import_stmt,
                    });
                    continue;
                };

                if let Err(source) = package.open(&module_path).await {
                    match source {
                        OpenError::NotFound(_) => {
                            diags.push(InvalidImport {
                                module_id: import_path,
                                import: import_stmt,
                            });
                            continue;
                        }
                        source => {
                            return Err(ResolveImportError::Open {
                                import_path,
                                package_name,
                                module_path,
                                source,
                            });
                        }
                    }
                }
            }
        }

        Ok(Diagnosed::new((), diags))
    }

    fn package_name_for_import(&self, import_path: &ModuleId) -> Option<ModuleId> {
        self.packages
            .keys()
            .filter(|package_name| import_path.starts_with(package_name))
            .max_by_key(|package_name| package_name.len())
            .cloned()
    }

    pub async fn evaluate(
        &mut self,
        module_id: &ModuleId,
        effects: tokio::sync::mpsc::UnboundedSender<crate::Effect>,
    ) -> Result<crate::Value, EvaluateError> {
        let Some(package_name) = self.package_name_for_import(module_id) else {
            return Err(EvaluateError::ModuleNotLoaded(module_id.clone()));
        };

        let Some(module_segments) = module_id.suffix_after(&package_name) else {
            return Err(EvaluateError::ModuleNotLoaded(module_id.clone()));
        };
        if module_segments.is_empty() {
            return Err(EvaluateError::MissingModulePath(module_id.clone()));
        }

        let module_path = module_segments
            .iter()
            .cloned()
            .collect::<ModuleId>()
            .to_path_buf_with_extension("scl");

        let Some(package) = self.packages.get_mut(&package_name) else {
            return Err(EvaluateError::ModuleNotLoaded(module_id.clone()));
        };
        let file_mod = package
            .open(&module_path)
            .await
            .map_err(|err| EvaluateError::Open(module_id.clone(), err))?;

        let mut eval = crate::Eval::new(effects);
        eval.eval_file_mod(file_mod)
            .map_err(|err| EvaluateError::Eval(module_id.clone(), err))
    }
}
