use std::path::PathBuf;
use std::sync::Arc;

use crate::output::{report_diagnostics, spawn_effect_printer};

pub async fn run_program(root: PathBuf, package: String) -> anyhow::Result<()> {
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

    let fs_pkg = Arc::new(sclc::v2::FsPackage::new(root, package_id.clone()));
    let finder = sclc::v2::build_default_finder(fs_pkg);

    let Some(asg) = report_diagnostics(sclc::v2::compile(finder, &entry_refs).await?) else {
        return Ok(());
    };

    let module_id = sclc::ModuleId::new(package_id.clone(), vec!["Main".to_string()]);

    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = sclc::EvalCtx::new(effects_tx, package_id.to_string());
    let effects_task = spawn_effect_printer(effects_rx);

    let results = sclc::v2::eval(&asg, ctx)?;
    if let Some(result) = results.modules.get(&module_id) {
        println!("{}", result.value);
    }

    effects_task.await?;
    Ok(())
}
