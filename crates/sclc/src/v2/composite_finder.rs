use std::sync::Arc;

use super::{LoadError, Package, PackageFinder};

/// A [`PackageFinder`] that tries multiple inner finders in sequence,
/// returning the first match.
pub struct CompositePackageFinder {
    finders: Vec<Arc<dyn PackageFinder>>,
}

impl CompositePackageFinder {
    pub fn new(finders: Vec<Arc<dyn PackageFinder>>) -> Self {
        Self { finders }
    }
}

#[async_trait::async_trait]
impl PackageFinder for CompositePackageFinder {
    async fn find(&self, raw_id: &[&str]) -> Result<Option<Arc<dyn Package>>, LoadError> {
        for finder in &self.finders {
            if let Some(pkg) = finder.find(raw_id).await? {
                return Ok(Some(pkg));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::path::Path;

    use crate::{PackageId, Value};

    use super::super::PackageEntity;
    use super::*;

    struct TestPackage(PackageId);

    #[async_trait::async_trait]
    impl Package for TestPackage {
        fn id(&self) -> PackageId {
            self.0.clone()
        }
        async fn lookup(&self, _path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
            Ok(None)
        }
        async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
            Err(LoadError::NotFound(path.display().to_string()))
        }
        fn register_externs(&self, _: &mut HashMap<String, Value>) {}
    }

    /// Helper: wraps a TestPackage in the Arc<P> blanket impl path.
    fn finder(id: &[&str]) -> Arc<dyn PackageFinder> {
        struct Finder(Arc<TestPackage>);

        #[async_trait::async_trait]
        impl PackageFinder for Finder {
            async fn find(&self, raw_id: &[&str]) -> Result<Option<Arc<dyn Package>>, LoadError> {
                // Reuse the Arc<P: Package> blanket impl logic.
                self.0.find(raw_id).await
            }
        }

        let pkg = Arc::new(TestPackage(PackageId::from(
            id.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )));
        Arc::new(Finder(pkg))
    }

    #[tokio::test]
    async fn first_match_wins() {
        let composite = CompositePackageFinder::new(vec![finder(&["A"]), finder(&["A"])]);

        let result = composite.find(&["A", "Foo"]).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id(), PackageId::from(["A"]));
    }

    #[tokio::test]
    async fn fallthrough_when_no_match() {
        let composite = CompositePackageFinder::new(vec![finder(&["A"])]);

        let result = composite.find(&["B", "Foo"]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn tries_finders_in_order() {
        let composite = CompositePackageFinder::new(vec![finder(&["A"]), finder(&["B"])]);

        let result = composite.find(&["B", "Foo"]).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id(), PackageId::from(["B"]));
    }
}
