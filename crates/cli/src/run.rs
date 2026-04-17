use std::path::PathBuf;
use std::sync::Arc;

use crate::output::{report_diagnostics, spawn_effect_printer};
use crate::resolver;

pub async fn run_program(root: PathBuf, package: String, git_server: String) -> anyhow::Result<()> {
    let package_id = resolver::parse_package_id(&package);

    let entry_segments: Vec<String> = package_id
        .as_slice()
        .iter()
        .cloned()
        .chain(std::iter::once("Main".to_string()))
        .collect();
    let entry_refs: Vec<&str> = entry_segments.iter().map(String::as_str).collect();

    let fs_pkg: Arc<dyn sclc::Package> = Arc::new(sclc::FsPackage::new(root, package_id.clone()));
    let default_finder = sclc::build_default_finder(Arc::clone(&fs_pkg));

    let resolved =
        resolver::resolve_package_deps(Arc::clone(&fs_pkg), default_finder.clone(), &git_server)
            .await?;

    let finder: Arc<dyn sclc::PackageFinder> = if resolved.is_empty() {
        default_finder
    } else {
        crate::cache::build_cached_finder(fs_pkg.clone(), &resolved)
    };

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
