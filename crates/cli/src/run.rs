use std::path::PathBuf;

use crate::output::{report_diagnostics, spawn_effect_printer};

pub async fn run_program(root: PathBuf, package: String) -> anyhow::Result<()> {
    let package_id = package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<sclc::PackageId>();
    let source = sclc::FsSource {
        root,
        package_id: package_id.clone(),
    };
    let Some(unit) = report_diagnostics(sclc::compile(source).await?) else {
        return Ok(());
    };

    let module_id = sclc::ModuleId::new(package_id.clone(), vec!["Main".to_string()]);

    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = sclc::EvalCtx::new(effects_tx, package_id.to_string());
    let effects_task = spawn_effect_printer(effects_rx);

    if let Some(result) = unit.eval(ctx)?.get(&module_id).cloned() {
        println!("{}", result.value);
    }

    effects_task.await?;
    Ok(())
}
