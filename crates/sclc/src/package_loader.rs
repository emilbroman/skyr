use std::sync::Arc;

use crate::{ModuleId, SourceError, SourceRepo};

/// A trait for dynamically discovering packages during import resolution.
///
/// When the compiler encounters an import that doesn't match any loaded
/// package, it calls [`load_package`] to give the environment a chance
/// to provide the source repository for that package.
///
/// This is the extension point for future features like loading packages
/// from foreign Git repositories.
///
/// [`load_package`]: PackageLoader::load_package
#[async_trait::async_trait]
pub trait PackageLoader: Send + Sync {
    /// Attempt to provide a source repo for an unresolved import path.
    ///
    /// The implementation should inspect the import path and, if it can
    /// identify a package that contains the target module, return its
    /// source repo. The returned [`SourceRepo::package_id()`] must be
    /// a prefix of `import_path` for the resolution to succeed.
    ///
    /// Return `Ok(None)` if the loader cannot resolve the import.
    async fn load_package(
        &self,
        import_path: &ModuleId,
    ) -> Result<Option<Arc<dyn SourceRepo>>, SourceError>;
}
