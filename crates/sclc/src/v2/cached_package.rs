use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::{PackageId, Value};

use super::{LoadError, Package, PackageEntity};

/// Memoizing wrapper around any [`Package`] impl.
///
/// Caches both `lookup` and `load` results. No invalidation — pure
/// memoization. Suitable for immutable sources (CDB) or short-lived
/// compilations.
pub struct CachedPackage<P> {
    inner: P,
    lookup_cache: RwLock<HashMap<PathBuf, Option<PackageEntity>>>,
    load_cache: RwLock<HashMap<PathBuf, Vec<u8>>>,
}

impl<P: Package> CachedPackage<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            lookup_cache: RwLock::new(HashMap::new()),
            load_cache: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl<P: Package> Package for CachedPackage<P> {
    fn id(&self) -> PackageId {
        self.inner.id()
    }

    async fn lookup(&self, path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
        {
            let cache = self.lookup_cache.read().unwrap();
            if let Some(entry) = cache.get(path) {
                return Ok(entry.clone().map(Cow::Owned));
            }
        }

        let result = self.inner.lookup(path).await?;
        let owned = result.map(|cow| cow.into_owned());

        let mut cache = self.lookup_cache.write().unwrap();
        cache.insert(path.to_path_buf(), owned.clone());
        Ok(owned.map(Cow::Owned))
    }

    async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
        {
            let cache = self.load_cache.read().unwrap();
            if let Some(data) = cache.get(path) {
                return Ok(Cow::Owned(data.clone()));
            }
        }

        let data = self.inner.load(path).await?.into_owned();

        let mut cache = self.load_cache.write().unwrap();
        cache.insert(path.to_path_buf(), data.clone());
        Ok(Cow::Owned(data))
    }

    fn register_externs(&self, externs: &mut HashMap<String, Value>) {
        self.inner.register_externs(externs);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Mock package that counts how many times `lookup` and `load` are called.
    /// Uses `Arc` internally so we can share it with the `CachedPackage`.
    struct CountingPackage {
        lookup_count: AtomicUsize,
        load_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl Package for CountingPackage {
        fn id(&self) -> PackageId {
            PackageId::from(["Test"])
        }

        async fn lookup(&self, _path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
            self.lookup_count.fetch_add(1, Ordering::SeqCst);
            let hash = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);
            Ok(Some(Cow::Owned(PackageEntity::File { hash })))
        }

        async fn load(&self, _path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
            self.load_count.fetch_add(1, Ordering::SeqCst);
            Ok(Cow::Owned(b"content".to_vec()))
        }
    }

    #[tokio::test]
    async fn caches_lookup_results() {
        let inner = Arc::new(CountingPackage {
            lookup_count: AtomicUsize::new(0),
            load_count: AtomicUsize::new(0),
        });
        let cached = CachedPackage::new(CountingProxy(Arc::clone(&inner)));
        let path = Path::new("test.scl");

        let _ = cached.lookup(path).await.unwrap();
        assert_eq!(inner.lookup_count.load(Ordering::SeqCst), 1);

        let _ = cached.lookup(path).await.unwrap();
        assert_eq!(inner.lookup_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn caches_load_results() {
        let inner = Arc::new(CountingPackage {
            lookup_count: AtomicUsize::new(0),
            load_count: AtomicUsize::new(0),
        });
        let cached = CachedPackage::new(CountingProxy(Arc::clone(&inner)));
        let path = Path::new("test.scl");

        let _ = cached.load(path).await.unwrap();
        assert_eq!(inner.load_count.load(Ordering::SeqCst), 1);

        let _ = cached.load(path).await.unwrap();
        assert_eq!(inner.load_count.load(Ordering::SeqCst), 1);
    }

    /// Thin wrapper that implements `Package` and delegates to an inner `Arc<CountingPackage>`.
    struct CountingProxy(Arc<CountingPackage>);

    #[async_trait::async_trait]
    impl Package for CountingProxy {
        fn id(&self) -> PackageId {
            self.0.id()
        }
        async fn lookup(&self, path: &Path) -> Result<Option<Cow<'_, PackageEntity>>, LoadError> {
            self.0.lookup(path).await
        }
        async fn load(&self, path: &Path) -> Result<Cow<'_, Vec<u8>>, LoadError> {
            self.0.load(path).await
        }
    }

    #[tokio::test]
    async fn delegates_id() {
        let cached = CachedPackage::new(CountingProxy(Arc::new(CountingPackage {
            lookup_count: AtomicUsize::new(0),
            load_count: AtomicUsize::new(0),
        })));
        assert_eq!(cached.id(), PackageId::from(["Test"]));
    }
}
