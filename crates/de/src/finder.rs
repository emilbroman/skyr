use std::sync::Arc;

/// Compose the finder chain used during compile: local user package →
/// cross-repo finder (if any) → CDB-backed cross-repo fallback for the
/// local environment → standard library.
pub(crate) fn build_full_finder(
    user_pkg: Arc<dyn sclc::Package>,
    cdb_client: cdb::Client,
    environment: ids::EnvironmentId,
    cross_repo_finder: Option<Arc<sclc::CrossRepoPackageFinder>>,
) -> Arc<dyn sclc::PackageFinder> {
    use sclc::CompositePackageFinder;

    let std_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::StdPackage::new());
    let cdb_finder: Arc<dyn sclc::PackageFinder> =
        Arc::new(sclc::CdbPackageFinder::new(cdb_client, environment));

    let mut finders: Vec<Arc<dyn sclc::PackageFinder>> = Vec::new();
    finders.push(wrap_pkg(user_pkg));
    if let Some(cr) = cross_repo_finder {
        finders.push(cr);
    }
    finders.push(cdb_finder);
    finders.push(wrap_pkg(std_pkg));

    Arc::new(CompositePackageFinder::new(finders))
}

pub(crate) fn wrap_pkg(pkg: Arc<dyn sclc::Package>) -> Arc<dyn sclc::PackageFinder> {
    struct PkgFinder(Arc<dyn sclc::Package>);

    #[async_trait::async_trait]
    impl sclc::PackageFinder for PkgFinder {
        async fn find(
            &self,
            raw_id: &[&str],
        ) -> Result<Option<Arc<dyn sclc::Package>>, sclc::LoadError> {
            let pkg_id = self.0.id();
            let segments = pkg_id.as_slice();
            if raw_id.len() >= segments.len()
                && raw_id[..segments.len()]
                    .iter()
                    .zip(segments.iter())
                    .all(|(a, b)| *a == b.as_str())
            {
                Ok(Some(Arc::clone(&self.0)))
            } else {
                Ok(None)
            }
        }
    }

    Arc::new(PkgFinder(pkg))
}
