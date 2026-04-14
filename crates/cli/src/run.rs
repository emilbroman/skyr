use std::path::PathBuf;
use std::sync::Arc;

use crate::output::{report_diagnostics, spawn_effect_printer};

pub async fn run_program(root: PathBuf, package: String, git_server: String) -> anyhow::Result<()> {
    let package_id = package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<sclc::PackageId>();

    let entry_segments: Vec<String> = package_id
        .as_slice()
        .iter()
        .cloned()
        .chain(std::iter::once("Main".to_string()))
        .collect();
    let entry_refs: Vec<&str> = entry_segments.iter().map(String::as_str).collect();

    let fs_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::FsPackage::new(root, package_id.clone()));

    let finder = resolve_dependencies(Arc::clone(&fs_pkg), &git_server).await?;

    let Some(asg) = report_diagnostics(sclc::compile(finder, &entry_refs).await?) else {
        return Ok(());
    };

    let module_id = sclc::ModuleId::new(package_id.clone(), vec!["Main".to_string()]);

    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = sclc::EvalCtx::new(
        effects_tx,
        package_id.to_string(),
        sclc::placeholder_deployment_qid(),
    );
    let effects_task = spawn_effect_printer(effects_rx);

    let results = sclc::eval(&asg, ctx)?;
    if let Some(result) = results.modules.get(&module_id) {
        println!("{}", result.value);
    }

    effects_task.await?;
    Ok(())
}

/// Resolve Package.scle dependencies and build a finder that includes
/// all cached packages. Falls back to the default finder if there are
/// no dependencies or if the user has no SSH credentials configured.
async fn resolve_dependencies(
    user_package: Arc<dyn sclc::Package>,
    git_server: &str,
) -> anyhow::Result<Arc<dyn sclc::PackageFinder>> {
    let default_finder = sclc::build_default_finder(Arc::clone(&user_package));

    // Check if a manifest exists before trying to set up git auth.
    let manifest = sclc::load_manifest(Arc::clone(&user_package), default_finder.clone()).await?;
    let Some(manifest) = manifest else {
        return Ok(default_finder);
    };
    if manifest.dependencies.is_empty() {
        return Ok(default_finder);
    }

    let git_client = crate::git_client::GitClient::from_config(git_server.to_string()).await?;

    let resolved =
        crate::resolver::resolve_all(Arc::clone(&user_package), default_finder, &git_client)
            .await?;

    if resolved.is_empty() {
        Ok(sclc::build_default_finder(user_package))
    } else {
        Ok(crate::cache::build_cached_finder(user_package, &resolved))
    }
}
