use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use thiserror::Error;

use crate::std::StdSourceRepo;
use crate::{
    AnySource, Diag, DiagList, Diagnosed, ImportStmt, Loc, ModuleId, OpenError, Package, SourceRepo,
};

#[derive(Clone, Default)]
pub struct Program<S> {
    packages: HashMap<ModuleId, Package<AnySource<S>>>,
}

impl<S> Program<S> {
    pub fn packages(&self) -> impl Iterator<Item = (&ModuleId, &Package<AnySource<S>>)> {
        self.packages.iter()
    }

    pub fn check_types(&self) -> Result<crate::Diagnosed<()>, crate::TypeCheckError>
    where
        S: SourceRepo,
    {
        crate::TypeChecker::new(self).check_program()
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
        let mut packages = HashMap::new();
        let std = StdSourceRepo::new();
        packages.insert(std.package_id(), Package::new(AnySource::Std(std)));
        Self { packages }
    }

    pub async fn open_package(&mut self, source: S) -> &mut Package<AnySource<S>> {
        let name = SourceRepo::package_id(&source);
        self.packages
            .entry(name)
            .or_insert_with(|| Package::new(AnySource::User(source)))
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
                        .map(|var| var.as_ref().name.clone())
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
                if self
                    .resolve_import(&import_path)
                    .await?
                    .unpack(&mut diags)
                    .is_none()
                {
                    diags.push(InvalidImport {
                        module_id: import_path,
                        import: import_stmt,
                    });
                }
            }
        }

        Ok(Diagnosed::new((), diags))
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

        let module_path = module_segments
            .iter()
            .cloned()
            .collect::<ModuleId>()
            .to_path_buf_with_extension("scl");

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

    pub async fn evaluate(
        &mut self,
        module_id: &ModuleId,
        eval: &crate::Eval,
    ) -> Result<Diagnosed<()>, EvaluateError> {
        let mut diags = DiagList::new();

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

        let file_mod = {
            let Some(package) = self.packages.get_mut(&package_name) else {
                return Err(EvaluateError::ModuleNotLoaded(module_id.clone()));
            };
            let open_result = package
                .open(&module_path)
                .await
                .map_err(|err| EvaluateError::Open(module_id.clone(), err))?
                .unpack(&mut diags);
            match open_result {
                Some(file_mod) => file_mod.clone(),
                None => return Ok(Diagnosed::new((), diags)),
            }
        };
        let imports = self.find_imports(&file_mod);

        let env = crate::EvalEnv::new()
            .with_module_id(module_id)
            .with_imports(&imports);
        eval.eval_file_mod(&env, &file_mod)
            .map_err(|err| EvaluateError::Eval(module_id.clone(), err))?;
        Ok(Diagnosed::new((), diags))
    }

    fn find_imports<'a>(
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
        let module_path = module_segments
            .iter()
            .cloned()
            .collect::<ModuleId>()
            .to_path_buf_with_extension("scl");
        package
            .modules()
            .find_map(|(path, file_mod)| (path == &module_path).then_some(file_mod))
    }
}
