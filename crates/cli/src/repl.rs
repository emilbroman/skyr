use std::collections::HashMap;

use rustyline::{DefaultEditor, error::ReadlineError};
use sclc::Diag;
use tokio::task;

use crate::fs_source::FsSource;

struct Repl {
    line_number: usize,
    bindings: HashMap<String, (sclc::Type, sclc::TrackedValue)>,
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
        let Some(repl_line) =
            Self::report(sclc::parse_repl_line(line, &module_id)).and_then(|line| line)
        else {
            return Ok(());
        };

        let Some(statement) = &repl_line.statement else {
            return Ok(());
        };

        let mut program = sclc::Program::<FsSource>::new();
        let type_env = Self::type_env(&self.bindings, &module_id);
        let pending_binding = match statement {
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
                let diagnosed = checker.check_expr(&type_env, &let_bind.expr, None)?;
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
                let diagnosed = checker.check_stmt(&type_env, statement)?;
                let Some(_) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&self.bindings, &module_id);
                let value = self.eval.eval_expr(&eval_env, expr)?;
                println!("{}", value.value);
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
        bindings: &'a HashMap<String, (sclc::Type, sclc::TrackedValue)>,
        module_id: &'a sclc::ModuleId,
    ) -> sclc::TypeEnv<'a> {
        bindings.iter().fold(
            sclc::TypeEnv::new().with_module_id(module_id),
            |env, (name, (ty, _))| env.with_local(name.as_str(), ty.clone()),
        )
    }

    fn eval_env<'a>(
        bindings: &'a HashMap<String, (sclc::Type, sclc::TrackedValue)>,
        module_id: &'a sclc::ModuleId,
    ) -> sclc::EvalEnv<'a> {
        bindings.iter().fold(
            sclc::EvalEnv::new().with_module_id(module_id),
            |env, (name, (_, value))| env.with_local(name.as_str(), value.clone()),
        )
    }
}

pub async fn run_repl() -> anyhow::Result<()> {
    let (effects_tx, mut effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let effects_task = task::spawn(async move {
        while let Some(effect) = effects_rx.recv().await {
            match effect {
                sclc::Effect::CreateResource { id, inputs, .. } => {
                    println!("CREATE {}:{} {:?}", id.ty, id.id, inputs);
                }
                sclc::Effect::UpdateResource { id, inputs, .. } => {
                    println!("UPDATE {}:{} {:?}", id.ty, id.id, inputs);
                }
                sclc::Effect::TouchResource { id, inputs, .. } => {
                    println!("TOUCH {}:{} {:?}", id.ty, id.id, inputs);
                }
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
