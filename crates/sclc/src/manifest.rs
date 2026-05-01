//! Cross-repo dependency manifests.
//!
//! Each repository may host a `Package.scle` at its root, declaring the
//! foreign repos it depends on (per-repo, not per-deployment). The manifest
//! is parsed and evaluated as an SCLE value of type `Std/Package.Manifest`:
//!
//! ```ignore
//! import Std/Package
//!
//! Package.Manifest
//!
//! {
//!     dependencies: #{
//!         "MyOrg/SomeRepo": "main",
//!         "MyOrg/Other":    "tag:v1.0.0",
//!         "MyOrg/Pinned":   "b50d18287a6a3b86c3f45e3a973a389784d353dd",
//!     }
//! }
//! ```
//!
//! The dependency map's values are interpreted per [`Specifier::parse`].

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use ids::RepoQid;
use thiserror::Error;

use crate::{
    AsgEvaluator, CompileError, CompositePackageFinder, DiagList, EvalCtx, GlobalKey, LoadError,
    Package, PackageFinder, Value, compile, wrap_as_finder,
};

/// Conventional path of a manifest within a repo.
pub const MANIFEST_FILENAME: &str = "Package.scle";

/// A parsed dependency specifier — see [`Specifier::parse`] for the
/// string-encoding rules.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Specifier {
    /// A branch — follow-the-channel; always resolves to the active
    /// deployment of that branch (volatile).
    Branch(String),
    /// A tag (encoded as `tag:<name>`).
    Tag(String),
    /// A 40-character hex commit hash.
    Hash(String),
}

impl Specifier {
    /// Parse a specifier string per the conventions defined in
    /// `CROSS_REPO_IMPORTS.md` §3:
    ///
    /// - 40 lowercase-hex characters → [`Specifier::Hash`].
    /// - `tag:<name>` → [`Specifier::Tag`].
    /// - anything else → [`Specifier::Branch`].
    pub fn parse(raw: &str) -> Self {
        if let Some(rest) = raw.strip_prefix("tag:") {
            Specifier::Tag(rest.to_string())
        } else if is_commit_hash(raw) {
            Specifier::Hash(raw.to_string())
        } else {
            Specifier::Branch(raw.to_string())
        }
    }

    /// The canonical string encoding (round-trips through [`Specifier::parse`]).
    pub fn to_raw(&self) -> String {
        match self {
            Specifier::Branch(name) => name.clone(),
            Specifier::Tag(name) => format!("tag:{name}"),
            Specifier::Hash(hex) => hex.clone(),
        }
    }

    /// Whether this specifier is volatile — i.e. its resolution may change
    /// over time without a corresponding manifest edit.
    pub fn is_volatile(&self) -> bool {
        matches!(self, Specifier::Branch(_) | Specifier::Tag(_))
    }
}

fn is_commit_hash(s: &str) -> bool {
    s.len() == 40
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// A parsed cross-repo dependency manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub dependencies: BTreeMap<RepoQid, Specifier>,
}

/// Errors when loading a manifest.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest contains diagnostics")]
    Invalid { diags: Vec<String> },

    #[error("manifest evaluated to an unexpected shape: {0}")]
    Shape(String),

    #[error("invalid dependency repo qualifier {0:?}: {1}")]
    InvalidRepoQid(String, ids::ParseIdError),

    #[error("load error: {0}")]
    Load(#[from] LoadError),

    #[error(transparent)]
    Compile(#[from] CompileError),

    #[error("evaluation error: {0}")]
    Eval(#[from] crate::EvalError),
}

/// Try to load and parse a manifest from `package`'s root.
///
/// Returns:
/// - `Ok(Some(Manifest))` if a `Package.scle` was found and parsed cleanly.
/// - `Ok(None)` if no `Package.scle` exists. (Repos without dependencies need
///   no manifest.)
/// - `Err(...)` for malformed manifests or I/O errors.
pub async fn load_manifest(
    package: Arc<dyn Package>,
    finder: Arc<dyn PackageFinder>,
) -> Result<Option<Manifest>, ManifestError> {
    // If the package doesn't contain a `Package.scle`, there's no manifest.
    let path = Path::new(MANIFEST_FILENAME);
    match package.lookup(path).await? {
        Some(_) => {}
        None => return Ok(None),
    }

    // Compose a finder that serves the manifest's own package (so `Self/...`
    // imports inside the manifest resolve) plus whatever the caller supplied
    // (stdlib, cross-repo, etc.).
    let pkg_id_segments: Vec<String> = package.id().as_slice().to_vec();
    let combined: Arc<dyn PackageFinder> = Arc::new(CompositePackageFinder::new(vec![
        wrap_as_finder(Arc::clone(&package)),
        finder,
    ]));

    // Run the standard compile pipeline. `Package.scle` is discovered
    // automatically via the loader's `.scl` / `.scle` probing.
    let entry: Vec<&str> = pkg_id_segments
        .iter()
        .map(String::as_str)
        .chain(std::iter::once("Package"))
        .collect();

    let mut diags = DiagList::new();
    let asg = compile(combined, &entry).await?.unpack(&mut diags);
    if diags.has_errors() {
        return Err(ManifestError::Invalid {
            diags: diags.iter().map(|d| d.to_string()).collect(),
        });
    }

    // Evaluate.
    let (effects_tx, _effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = EvalCtx::new(
        effects_tx,
        "manifest",
        crate::placeholder_deployment_qid(),
        crate::placeholder_region(),
    );
    let (_results, env) = AsgEvaluator::new(&asg, ctx).eval()?;

    let raw_id: Vec<String> = entry.iter().map(|s| s.to_string()).collect();
    let value = env
        .get(&GlobalKey::ModuleValue(raw_id))
        .cloned()
        .ok_or_else(|| ManifestError::Shape("manifest produced no value".into()))?;

    parse_manifest_value(value.value)
}

fn parse_manifest_value(value: Value) -> Result<Option<Manifest>, ManifestError> {
    let Value::Record(rec) = value else {
        return Err(ManifestError::Shape(format!(
            "manifest must be a record, got {value:?}"
        )));
    };

    let deps_value = rec.get("dependencies");
    let Value::Dict(dict) = deps_value else {
        return Err(ManifestError::Shape(format!(
            "manifest.dependencies must be a dict, got {deps_value:?}"
        )));
    };

    let mut dependencies = BTreeMap::new();
    for (k, v) in dict.iter() {
        let Value::Str(key_str) = k else {
            return Err(ManifestError::Shape(format!(
                "dependency key must be a string, got {k:?}"
            )));
        };
        let Value::Str(spec_str) = v else {
            return Err(ManifestError::Shape(format!(
                "dependency value must be a string, got {v:?}"
            )));
        };
        let repo_qid: RepoQid = key_str
            .parse()
            .map_err(|e| ManifestError::InvalidRepoQid(key_str.clone(), e))?;
        dependencies.insert(repo_qid, Specifier::parse(spec_str));
    }

    Ok(Some(Manifest { dependencies }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InMemoryPackage, PackageId, build_default_finder};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn finder() -> Arc<dyn PackageFinder> {
        let user_pkg = Arc::new(InMemoryPackage::new(
            PackageId::from(["__ManifestTestUser"]),
            HashMap::new(),
        ));
        build_default_finder(user_pkg)
    }

    #[test]
    fn specifier_parse_branch() {
        assert_eq!(Specifier::parse("main"), Specifier::Branch("main".into()));
        assert_eq!(
            Specifier::parse("feature/x"),
            Specifier::Branch("feature/x".into())
        );
    }

    #[test]
    fn specifier_parse_tag() {
        assert_eq!(Specifier::parse("tag:v1.0"), Specifier::Tag("v1.0".into()));
    }

    #[test]
    fn specifier_parse_hash() {
        let hash = "b50d18287a6a3b86c3f45e3a973a389784d353dd";
        assert_eq!(Specifier::parse(hash), Specifier::Hash(hash.into()));
    }

    #[test]
    fn specifier_round_trips() {
        for raw in [
            "main",
            "tag:v1.0",
            "b50d18287a6a3b86c3f45e3a973a389784d353dd",
        ] {
            assert_eq!(Specifier::parse(raw).to_raw(), raw);
        }
    }

    #[test]
    fn specifier_volatility() {
        assert!(Specifier::parse("main").is_volatile());
        assert!(Specifier::parse("tag:v1").is_volatile());
        assert!(!Specifier::parse("b50d18287a6a3b86c3f45e3a973a389784d353dd").is_volatile());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn load_returns_none_when_no_manifest() {
        let pkg = InMemoryPackage::empty(PackageId::from(["NoManifest"]));
        let result = load_manifest(Arc::new(pkg), finder()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn load_parses_simple_manifest() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from(MANIFEST_FILENAME),
            br#"
import Std/Package

Package.Manifest

{
    dependencies: #{
        "MyOrg/Repo": "main",
        "MyOrg/Other": "tag:v1.0",
        "MyOrg/Pinned": "b50d18287a6a3b86c3f45e3a973a389784d353dd",
    }
}
"#
            .to_vec(),
        );
        let pkg = InMemoryPackage::new(PackageId::from(["WithManifest"]), files);

        let manifest = load_manifest(Arc::new(pkg), finder())
            .await
            .expect("load failed")
            .expect("manifest should be present");

        assert_eq!(manifest.dependencies.len(), 3);
        assert_eq!(
            manifest.dependencies.get(&"MyOrg/Repo".parse().unwrap()),
            Some(&Specifier::Branch("main".into()))
        );
        assert_eq!(
            manifest.dependencies.get(&"MyOrg/Other".parse().unwrap()),
            Some(&Specifier::Tag("v1.0".into()))
        );
        assert_eq!(
            manifest.dependencies.get(&"MyOrg/Pinned".parse().unwrap()),
            Some(&Specifier::Hash(
                "b50d18287a6a3b86c3f45e3a973a389784d353dd".into()
            ))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn load_rejects_malformed_manifest() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from(MANIFEST_FILENAME),
            b"this is not valid scle".to_vec(),
        );
        let pkg = InMemoryPackage::new(PackageId::from(["BadManifest"]), files);

        let err = load_manifest(Arc::new(pkg), finder())
            .await
            .expect_err("expected manifest load to fail");
        assert!(matches!(err, ManifestError::Invalid { .. }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn load_rejects_invalid_repo_qid() {
        let mut files = HashMap::new();
        files.insert(
            PathBuf::from(MANIFEST_FILENAME),
            br#"
import Std/Package

Package.Manifest

{ dependencies: #{ "not-a-qid": "main" } }
"#
            .to_vec(),
        );
        let pkg = InMemoryPackage::new(PackageId::from(["BadDep"]), files);

        let err = load_manifest(Arc::new(pkg), finder())
            .await
            .expect_err("expected manifest load to fail");
        assert!(
            matches!(err, ManifestError::InvalidRepoQid(_, _)),
            "got {err:?}"
        );
    }
}
