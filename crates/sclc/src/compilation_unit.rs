use std::collections::HashMap;
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

    /// Create a compilation unit from an existing Program, syncing all modules
    /// and caches. Used for backward compatibility where callers still work
    /// with `Program` directly (e.g., REPL, stdlib_types).
    pub fn from_program(program: &Program) -> Self {
        let mut unit = Self {
            modules: HashMap::new(),
            path_hashes: HashMap::new(),
            children_cache: HashMap::new(),
            externs: HashMap::new(),
            program: program.clone(),
        };
        unit.sync_modules_from_program();

        // Copy path hashes
        for (key, hash) in program.path_hashes() {
            unit.path_hashes.insert(key.clone(), *hash);
        }

        // Copy children caches
        for (package_id, package) in program.packages() {
            for (path, children) in package.children_entries() {
                unit.children_cache
                    .insert((package_id.clone(), path.clone()), children.clone());
            }
        }

        // Collect externs
        for (_package_id, package) in program.packages() {
            for (name, value) in package.source().externs() {
                unit.externs.insert(name, value);
            }
        }

        unit
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

        // Load the entry module
        let package = self.program.packages_mut().get_mut(&entry.package);
        if let Some(package) = package {
            let module_path = entry.to_path_buf_with_extension("scl");
            match package.open(&module_path).await {
                Ok(diagnosed) => {
                    let file_mod = diagnosed.unpack(&mut diags);
                    if let Some(file_mod) = file_mod {
                        self.modules.insert(entry.clone(), file_mod.clone());
                    }
                }
                Err(crate::OpenError::NotFound(_)) => {
                    // Entry module not found — not an error, just no modules to load
                    return Ok(Diagnosed::new((), diags));
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Resolve all imports (this populates Package.files via the existing loop)
        self.program.resolve_imports().await?.unpack(&mut diags);

        // Resolve path expressions (populates path_hashes)
        self.program.resolve_paths().await?.unpack(&mut diags);

        // Now sync: copy all parsed modules from Program's packages into our flat map
        self.sync_modules_from_program();

        // Copy path hashes
        for (key, hash) in self.program.path_hashes() {
            self.path_hashes.insert(key.clone(), *hash);
        }

        // Copy children caches
        for (package_id, package) in self.program.packages() {
            for (path, children) in package.children_entries() {
                self.children_cache
                    .insert((package_id.clone(), path.clone()), children.clone());
            }
        }

        // Collect externs from all loaded packages
        for (_package_id, package) in self.program.packages() {
            for (name, value) in package.source().externs() {
                self.externs.insert(name, value);
            }
        }

        Ok(Diagnosed::new((), diags))
    }

    /// Copy all parsed modules from the Program's packages into the flat module map.
    fn sync_modules_from_program(&mut self) {
        for (package_id, package) in self.program.packages() {
            for (path, file_mod) in package.modules() {
                let module_id = module_id_for_path(package_id, path);
                self.modules
                    .entry(module_id)
                    .or_insert_with(|| file_mod.clone());
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

    /// Access the underlying Program (for backward compatibility during migration).
    pub fn program(&self) -> &Program {
        &self.program
    }

    /// Mutable access to the underlying Program.
    pub fn program_mut(&mut self) -> &mut Program {
        &mut self.program
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

    pub fn repl(
        self,
        program: Program,
        effects_tx: tokio::sync::mpsc::UnboundedSender<crate::Effect>,
        namespace: String,
    ) -> crate::Repl {
        crate::Repl::from_parts(self, program, effects_tx, namespace)
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

    fn split_import_segments(&self, segments: &[String]) -> Option<ModuleId> {
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

/// Compute a module ID from a package ID and a file path within that package.
fn module_id_for_path(package_id: &PackageId, path: &Path) -> ModuleId {
    let mut path_segments = Vec::new();
    if let Some(parent) = path.parent() {
        for segment in parent.components() {
            if let std::path::Component::Normal(part) = segment {
                path_segments.push(part.to_string_lossy().into_owned());
            }
        }
    }

    if let Some(stem) = path.file_stem() {
        path_segments.push(stem.to_string_lossy().into_owned());
    }

    ModuleId::new(package_id.clone(), path_segments)
}
