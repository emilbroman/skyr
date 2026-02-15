use clap::Parser;
use rustyline::{DefaultEditor, error::ReadlineError};
use std::collections::HashMap;
use tokio::task;

struct Repl {
    line_number: usize,
    bindings: HashMap<String, (sclc::Type, sclc::Value)>,
    effects_tx: tokio::sync::mpsc::UnboundedSender<sclc::Effect>,
}

impl Repl {
    fn new(effects_tx: tokio::sync::mpsc::UnboundedSender<sclc::Effect>) -> Self {
        Self {
            line_number: 0,
            bindings: HashMap::new(),
            effects_tx,
        }
    }

    fn process(&mut self, line: &str) -> anyhow::Result<()> {
        self.line_number += 1;

        let module_id = [format!("Repl{}", self.line_number), String::from("Main")]
            .into_iter()
            .collect::<sclc::ModuleId>();
        let Some(repl_line) = Self::report(sclc::parse_repl_line(line, &module_id)?) else {
            return Ok(());
        };

        let type_env = self.type_env(&module_id);
        let checker = sclc::TypeChecker;
        let pending_binding = match &repl_line.statement {
            sclc::ModStmt::Let(let_bind) => {
                let diagnosed = checker.check_expr(&type_env, &let_bind.expr)?;
                let Some(ty) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = self.eval_env(&module_id);
                let mut eval = sclc::Eval::new(self.effects_tx.clone());
                let value = eval.eval_expr(&eval_env, &let_bind.expr)?;
                println!("{} : {}", let_bind.var.name, ty);
                Some((let_bind.var.name.clone(), (ty, value)))
            }
            sclc::ModStmt::Expr(expr) => {
                let diagnosed = checker.check_stmt(&type_env, &repl_line.statement)?;
                let Some(()) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = self.eval_env(&module_id);
                let mut eval = sclc::Eval::new(self.effects_tx.clone());
                let value = eval.eval_expr(&eval_env, expr)?;
                println!("{value}");
                None
            }
            stmt => {
                let diagnosed = checker.check_stmt(&type_env, stmt)?;
                let Some(()) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = self.eval_env(&module_id);
                let mut eval = sclc::Eval::new(self.effects_tx.clone());
                eval.eval_stmt(&eval_env, stmt)?;
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

    fn type_env<'a>(&'a self, module_id: &'a sclc::ModuleId) -> sclc::TypeEnv<'a> {
        self.bindings.iter().fold(
            sclc::TypeEnv::new().with_module_id(module_id),
            |env, (name, (ty, _))| env.with_local(name.as_str(), ty.clone()),
        )
    }

    fn eval_env<'a>(&'a self, module_id: &'a sclc::ModuleId) -> sclc::EvalEnv<'a> {
        self.bindings.iter().fold(
            sclc::EvalEnv::new().with_module_id(module_id),
            |env, (name, (_, value))| env.with_local(name.as_str(), value.clone()),
        )
    }
}

#[derive(Parser)]
enum Program {
    Repl,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Program::parse() {
        Program::Repl => {
            run_repl().await?;
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
    let mut repl = Repl::new(effects_tx.clone());
    let mut editor = DefaultEditor::new()?;

    loop {
        match editor.readline("scl> ") {
            Ok(line) => {
                editor.add_history_entry(&line)?;

                repl.process(&line)?;
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }

    drop(effects_tx);
    effects_task.await?;

    Ok(())
}
