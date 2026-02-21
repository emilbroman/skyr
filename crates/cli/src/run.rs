use std::path::PathBuf;

use tokio::task;

use crate::fs_source::FsSource;

pub async fn run_program(root: PathBuf, package: String) -> anyhow::Result<()> {
    let package_id = package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<sclc::ModuleId>();
    let source = FsSource {
        root,
        package_id: package_id.clone(),
    };
    let Some(mut program) = report(sclc::compile(source).await?) else {
        return Ok(());
    };

    let module_id = package_id
        .as_slice()
        .iter()
        .cloned()
        .chain(std::iter::once(String::from("Main")))
        .collect::<sclc::ModuleId>();

    let (effects_tx, mut effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let eval = sclc::Eval::new::<FsSource>(effects_tx);
    let effects_task = task::spawn(async move {
        while let Some(effect) = effects_rx.recv().await {
            match effect {
                sclc::Effect::CreateResource { id, inputs, .. } => {
                    println!("CREATE {}:{} {:?}", id.ty, id.id, inputs);
                }
                sclc::Effect::UpdateResource { id, inputs, .. } => {
                    println!("UPDATE {}:{} {:?}", id.ty, id.id, inputs);
                }
            }
        }
    });

    program.evaluate(&module_id, &eval).await?;

    effects_task.await?;
    Ok(())
}

fn report<T>(diagnosed: sclc::Diagnosed<T>) -> Option<T> {
    for diag in diagnosed.diags().iter() {
        let (module_id, span) = diag.locate();
        println!("[{:?}] {module_id}:{span}: {diag}", diag.level());
    }

    if diagnosed.diags().has_errors() {
        return None;
    }

    Some(diagnosed.into_inner())
}
