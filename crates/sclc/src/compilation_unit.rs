use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{ChildEntry, DiagList, Diagnosed, FileMod, ModuleId, PackageId, Program, Value};

/// A fully-resolved compilation unit containing all parsed modules and metadata
/// needed for type checking and evaluation.
///
/// The `CompilationUnit` owns all parsed `FileMod`s in a flat map keyed by
/// `ModuleId`, along with path hashes, directory listings, and extern function
/// implementations. It serves as the immutable context for type checking and
/// evaluation after the resolution phase completes.
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
