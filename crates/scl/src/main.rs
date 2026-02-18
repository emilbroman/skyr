use clap::Parser;
use rustyline::{DefaultEditor, error::ReadlineError};
use sclc::Diag;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::task;

struct FsSource {
    root: PathBuf,
    package_id: sclc::ModuleId,
}

impl sclc::SourceRepo for FsSource {
    type Err = std::io::Error;

    fn package_id(&self) -> sclc::ModuleId {
        self.package_id.clone()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        match tokio::fs::read(self.root.join(path)).await {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }
}

struct Repl {
    line_number: usize,
    bindings: HashMap<String, (sclc::Type, sclc::Value)>,
    eval: sclc::Eval,
}

impl Repl {
    fn new(eval: sclc::Eval) -> Self {
        Self {
            line_number: 0,
            bindings: HashMap::new(),
            eval,
        }
    }

    async fn process(&mut self, line: &str) -> anyhow::Result<()> {
        self.line_number += 1;

        let module_id = [format!("Repl{}", self.line_number)].into();
        let Some(repl_line) = Self::report(sclc::parse_repl_line(line, &module_id)?) else {
            return Ok(());
        };

        let mut program = sclc::Program::<FsSource>::new();
        let type_env = Self::type_env(&self.bindings, &module_id);
        let pending_binding = match &repl_line.statement {
            sclc::ModStmt::Import(import_stmt) => {
                let import_path = import_stmt
                    .as_ref()
                    .vars
                    .iter()
                    .map(|var| var.as_ref().name.clone())
                    .collect::<sclc::ModuleId>();
                let Some(alias) = import_stmt
                    .as_ref()
                    .vars
                    .last()
                    .map(|var| var.as_ref().name.clone())
                else {
                    return Ok(());
                };

                let Some(file_mod) = program.resolve_import(&import_path).await?.cloned() else {
                    let diag = sclc::InvalidImport {
                        module_id: import_path,
                        import: import_stmt.clone(),
                    };
                    let (module_id, span) = diag.locate();
                    println!("[{:?}] {module_id}:{span}: {diag}", diag.level());
                    return Ok(());
                };

                let checker = sclc::TypeChecker::new(&program);
                let type_env = sclc::TypeEnv::new().with_module_id(&import_path);
                let diagnosed_ty = checker.check_file_mod(&type_env, &file_mod)?;
                let Some(ty) = Self::report(diagnosed_ty) else {
                    return Ok(());
                };

                let eval_env = sclc::EvalEnv::new().with_module_id(&import_path);
                let value = self.eval.eval_file_mod(&eval_env, &file_mod)?;
                Some((alias, (ty, value)))
            }
            sclc::ModStmt::Let(let_bind) => {
                let checker = sclc::TypeChecker::new(&program);
                let diagnosed = checker.check_expr(&type_env, &let_bind.expr)?;
                let Some(ty) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&self.bindings, &module_id);
                let value = self.eval.eval_expr(&eval_env, &let_bind.expr)?;
                println!("{} : {}", let_bind.var.name, ty);
                Some((let_bind.var.name.clone(), (ty, value)))
            }
            sclc::ModStmt::Expr(expr) => {
                let checker = sclc::TypeChecker::new(&program);
                let diagnosed = checker.check_stmt(&type_env, &repl_line.statement)?;
                let Some(_) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&self.bindings, &module_id);
                let value = self.eval.eval_expr(&eval_env, expr)?;
                println!("{value}");
                None
            }
            stmt => {
                let checker = sclc::TypeChecker::new(&program);
                let diagnosed = checker.check_stmt(&type_env, stmt)?;
                let Some(_) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&self.bindings, &module_id);
                let _ = self.eval.eval_stmt(&eval_env, stmt)?;
                None
            }
        };

        if let Some((name, binding)) = pending_binding {
            self.bindings.insert(name, binding);
        }

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

    fn type_env<'a>(
        bindings: &'a HashMap<String, (sclc::Type, sclc::Value)>,
        module_id: &'a sclc::ModuleId,
    ) -> sclc::TypeEnv<'a> {
        bindings.iter().fold(
            sclc::TypeEnv::new().with_module_id(module_id),
            |env, (name, (ty, _))| env.with_local(name.as_str(), ty.clone()),
        )
    }

    fn eval_env<'a>(
        bindings: &'a HashMap<String, (sclc::Type, sclc::Value)>,
        module_id: &'a sclc::ModuleId,
    ) -> sclc::EvalEnv<'a> {
        bindings.iter().fold(
            sclc::EvalEnv::new().with_module_id(module_id),
            |env, (name, (_, value))| env.with_local(name.as_str(), value.clone()),
        )
    }
}

#[derive(Parser)]
enum Program {
    Repl,
    Run {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value = "Local")]
        package: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Program::parse() {
        Program::Repl => {
            run_repl().await?;
        }
        Program::Run { root, package } => {
            run_program(root, package).await?;
        }
    }

    Ok(())
}

async fn run_repl() -> anyhow::Result<()> {
    let (effects_tx, mut effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let effects_task = task::spawn(async move {
        while let Some(effect) = effects_rx.recv().await {
            match effect {
                sclc::Effect::Print(value) => println!("{value}"),
            }
        }
    });
    let eval = sclc::Eval::new::<FsSource>(effects_tx);
    let mut repl = Repl::new(eval);
    let mut editor = DefaultEditor::new()?;

    loop {
        match editor.readline("scl> ") {
            Ok(line) => {
                editor.add_history_entry(&line)?;

                repl.process(&line).await?;
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }

    drop(repl);
    effects_task.await?;

    Ok(())
}

async fn run_program(root: PathBuf, package: String) -> anyhow::Result<()> {
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
    let effects_task = task::spawn(async move {
        while let Some(effect) = effects_rx.recv().await {
            match effect {
                sclc::Effect::Print(value) => println!("{value}"),
            }
        }
    });

    program.evaluate(&module_id, effects_tx).await?;

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
