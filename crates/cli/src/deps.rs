//! `skyr deps` — manage cross-repo dependencies in `./Package.scle`.
//!
//! v1 supports `list`, `add`, and `rm`. `update` and `pin` require server
//! access to resolve a branch/tag to a commit hash; they will land alongside
//! the API endpoints for cross-repo resolution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, anyhow};
use clap::{Args, Subcommand};
use ids::RepoQid;

const MANIFEST_FILENAME: &str = "Package.scle";

#[derive(Args, Debug)]
pub struct DepsArgs {
    #[command(subcommand)]
    command: DepsCommand,
}

#[derive(Subcommand, Debug)]
enum DepsCommand {
    /// List the dependencies declared in `./Package.scle`.
    List,
    /// Add a dependency, or replace an existing one.
    Add {
        /// The dependency repo, formatted as `Org/Repo`.
        repo: String,
        /// A branch name (e.g. `main`), `tag:<name>`, or 40-character commit hash.
        spec: String,
    },
    /// Remove a dependency.
    Rm {
        /// The dependency repo, formatted as `Org/Repo`.
        repo: String,
    },
}

pub async fn run_deps(args: DepsArgs, _format: crate::output::OutputFormat) -> anyhow::Result<()> {
    match args.command {
        DepsCommand::List => list().await,
        DepsCommand::Add { repo, spec } => add(repo, spec).await,
        DepsCommand::Rm { repo } => rm(repo).await,
    }
}

async fn list() -> anyhow::Result<()> {
    let manifest = read_manifest().await?.unwrap_or_default();
    if manifest.dependencies.is_empty() {
        println!("(no dependencies)");
        return Ok(());
    }
    for (repo, spec) in &manifest.dependencies {
        println!("{repo} = {}", spec.to_raw());
    }
    Ok(())
}

async fn add(repo: String, spec: String) -> anyhow::Result<()> {
    let repo_qid: RepoQid = repo
        .parse()
        .with_context(|| format!("invalid Org/Repo: {repo}"))?;
    let mut manifest = read_manifest().await?.unwrap_or_default();
    let specifier = sclc::Specifier::parse(&spec);
    manifest
        .dependencies
        .insert(repo_qid.clone(), specifier.clone());
    write_manifest(&manifest)?;
    println!("Set {repo_qid} = {}", specifier.to_raw());
    Ok(())
}

async fn rm(repo: String) -> anyhow::Result<()> {
    let repo_qid: RepoQid = repo
        .parse()
        .with_context(|| format!("invalid Org/Repo: {repo}"))?;
    let mut manifest = read_manifest().await?.unwrap_or_default();
    if manifest.dependencies.remove(&repo_qid).is_none() {
        return Err(anyhow!("{repo_qid} is not a dependency"));
    }
    write_manifest(&manifest)?;
    println!("Removed {repo_qid}");
    Ok(())
}

/// Read the local `Package.scle`, returning `Ok(None)` if it doesn't exist.
async fn read_manifest() -> anyhow::Result<Option<sclc::Manifest>> {
    let path = PathBuf::from(MANIFEST_FILENAME);
    if !path.exists() {
        return Ok(None);
    }
    let source = std::fs::read_to_string(&path)?;
    let pkg: Arc<dyn sclc::Package> = Arc::new(sclc::InMemoryPackage::new(
        sclc::PackageId::from(["__SkyrDepsCli"]),
        std::iter::once((PathBuf::from(MANIFEST_FILENAME), source.into_bytes())).collect(),
    ));
    let finder = sclc::build_default_finder(Arc::clone(&pkg));
    let manifest = sclc::load_manifest(pkg, finder)
        .await
        .context("failed to load Package.scle")?;
    Ok(manifest)
}

/// Write a manifest to `./Package.scle`, replacing any existing contents.
fn write_manifest(manifest: &sclc::Manifest) -> anyhow::Result<()> {
    let source = render_manifest(&manifest.dependencies);
    std::fs::write(Path::new(MANIFEST_FILENAME), source)?;
    Ok(())
}

/// Render a dependency map as a canonical SCLE source. Loses any comments or
/// non-trivial structure that may have been in the original file.
fn render_manifest(deps: &BTreeMap<RepoQid, sclc::Specifier>) -> String {
    let mut out = String::from("import Std/Package\n\nPackage.Manifest\n\n");
    if deps.is_empty() {
        out.push_str("{\n\tdependencies: #{},\n}\n");
        return out;
    }
    out.push_str("{\n\tdependencies: #{\n");
    for (repo, spec) in deps {
        out.push_str(&format!("\t\t\"{repo}\": \"{}\",\n", spec.to_raw()));
    }
    out.push_str("\t},\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_empty_manifest() {
        let out = render_manifest(&BTreeMap::new());
        assert!(out.contains("dependencies: #{}"));
        assert!(out.starts_with("import Std/Package\n"));
    }

    #[test]
    fn render_with_deps() {
        let mut deps = BTreeMap::new();
        deps.insert(
            "MyOrg/Repo".parse().unwrap(),
            sclc::Specifier::Branch("main".into()),
        );
        deps.insert(
            "MyOrg/Other".parse().unwrap(),
            sclc::Specifier::Tag("v1.0".into()),
        );
        let out = render_manifest(&deps);
        assert!(out.contains("\"MyOrg/Other\": \"tag:v1.0\""));
        assert!(out.contains("\"MyOrg/Repo\": \"main\""));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn rendered_manifest_round_trips() {
        let mut deps = BTreeMap::new();
        deps.insert(
            "MyOrg/Repo".parse().unwrap(),
            sclc::Specifier::Branch("main".into()),
        );
        deps.insert(
            "MyOrg/Pinned".parse().unwrap(),
            sclc::Specifier::Hash("b50d18287a6a3b86c3f45e3a973a389784d353dd".into()),
        );
        let source = render_manifest(&deps);

        // Re-parse via the manifest loader using an in-memory package.
        let pkg: Arc<dyn sclc::Package> = Arc::new(sclc::InMemoryPackage::new(
            sclc::PackageId::from(["__Test"]),
            std::iter::once((PathBuf::from(MANIFEST_FILENAME), source.into_bytes())).collect(),
        ));
        let finder = sclc::build_default_finder(Arc::clone(&pkg));
        let parsed = sclc::load_manifest(pkg, finder)
            .await
            .expect("manifest should load")
            .expect("manifest should be present");
        assert_eq!(parsed.dependencies, deps);
    }
}
