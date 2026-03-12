use std::cell::RefCell;
use std::collections::HashMap;

use rustyline::completion::{self, Candidate};
use rustyline::error::ReadlineError;
use rustyline::highlight;
use rustyline::hint;
use rustyline::validate;
use rustyline::{Context, Editor, Helper};
use sclc::Diag;
use tokio::task;

use crate::fs_source::FsSource;

struct ReplHelper {
    state: RefCell<CompletionState>,
}

struct CompletionState {
    bindings: HashMap<String, (sclc::Type, sclc::TrackedValue)>,
    type_defs: HashMap<String, sclc::Type>,
}

struct CompletionEntry(String);

impl Candidate for CompletionEntry {
    fn display(&self) -> &str {
        &self.0
    }

    fn replacement(&self) -> &str {
        &self.0
    }
}

impl ReplHelper {
    fn new() -> Self {
        Self {
            state: RefCell::new(CompletionState {
                bindings: HashMap::new(),
                type_defs: HashMap::new(),
            }),
        }
    }
}

impl completion::Completer for ReplHelper {
    type Candidate = CompletionEntry;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        // Convert byte offset to 1-based character position
        let char_pos = line[..pos].chars().count() as u32 + 1;
        let cursor = sclc::Cursor::new(sclc::Position::new(1, char_pos));
        let cursor_info = cursor.info();

        let module_id: sclc::ModuleId = ["ReplComplete"].into();

        // Parse with cursor (clone so we can also pass it to the type env)
        let diagnosed = sclc::parse_repl_line_with_cursor(line, &module_id, Some(cursor.clone()));
        let Some(repl_line) = diagnosed.into_inner() else {
            return Ok((0, Vec::new()));
        };

        let Some(statement) = &repl_line.statement else {
            return Ok((0, Vec::new()));
        };

        // Type-check to populate cursor info
        let state = self.state.borrow();
        let program = sclc::Program::<FsSource>::new();
        let type_env =
            Repl::type_env(&state.bindings, &state.type_defs, &module_id).with_cursor(cursor);
        let checker = sclc::TypeChecker::new(&program);
        let _ = checker.check_stmt(&type_env, statement);

        // Extract completion candidates
        let info = cursor_info.lock().unwrap();
        if info.completion_candidates.is_empty() {
            return Ok((0, Vec::new()));
        }

        // Find the start of the word being completed (scan back from pos)
        let start = line[..pos]
            .rfind(|c: char| !c.is_alphanumeric())
            .map(|i| i + line[i..].chars().next().map_or(0, char::len_utf8))
            .unwrap_or(0);

        let candidates =
            info.completion_candidates
                .iter()
                .map(|c| match c {
                    sclc::CompletionCandidate::Var(name)
                    | sclc::CompletionCandidate::Member(name) => CompletionEntry(name.clone()),
                })
                .collect();

        Ok((start, candidates))
    }
}

impl hint::Hinter for ReplHelper {
    type Hint = String;
}

impl highlight::Highlighter for ReplHelper {}
impl validate::Validator for ReplHelper {}
impl Helper for ReplHelper {}

struct Repl {
    line_number: usize,
    eval: sclc::Eval,
}

impl Repl {
    fn new(eval: sclc::Eval) -> Self {
        Self {
            line_number: 0,
            eval,
        }
    }

    async fn process(&mut self, line: &str, helper: &ReplHelper) -> anyhow::Result<()> {
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

        // Handle imports separately to avoid holding the RefCell borrow across .await
        if let sclc::ModStmt::Import(import_stmt) = statement {
            return self.process_import(import_stmt, helper).await;
        }

        let mut state = helper.state.borrow_mut();
        let program = sclc::Program::<FsSource>::new();
        let type_env = Self::type_env(&state.bindings, &state.type_defs, &module_id);
        let pending_binding = match statement {
            sclc::ModStmt::Import(_) => unreachable!(),
            sclc::ModStmt::Let(let_bind) => {
                let checker = sclc::TypeChecker::new(&program);
                let diagnosed = checker.check_global_let_bind(&type_env, let_bind)?;
                let Some(ty) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&state.bindings, &module_id);
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

                let eval_env = Self::eval_env(&state.bindings, &module_id);
                let value = self.eval.eval_expr(&eval_env, expr)?;
                println!("{}", value.value);
                None
            }
            sclc::ModStmt::Export(let_bind) => {
                let checker = sclc::TypeChecker::new(&program);
                let diagnosed = checker.check_global_let_bind(&type_env, let_bind)?;
                let Some(ty) = Self::report(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&state.bindings, &module_id);
                let value = self.eval.eval_expr(&eval_env, &let_bind.expr)?;
                println!("{} : {}", let_bind.var.name, ty);
                Some((let_bind.var.name.clone(), (ty, value)))
            }
            sclc::ModStmt::TypeDef(type_def) | sclc::ModStmt::ExportTypeDef(type_def) => {
                let checker = sclc::TypeChecker::new(&program);
                let Some(ty) = Self::report(checker.resolve_type_def(&type_env, type_def)) else {
                    return Ok(());
                };

                state.type_defs.insert(type_def.var.name.clone(), ty);
                None
            }
        };

        if let Some((name, binding)) = pending_binding {
            state.bindings.insert(name, binding);
        }

        Ok(())
    }

    async fn process_import(
        &mut self,
        import_stmt: &sclc::Loc<sclc::ImportStmt>,
        helper: &ReplHelper,
    ) -> anyhow::Result<()> {
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

        let mut program = sclc::Program::<FsSource>::new();
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

        helper
            .state
            .borrow_mut()
            .bindings
            .insert(alias, (ty, value));

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
        type_defs: &'a HashMap<String, sclc::Type>,
        module_id: &'a sclc::ModuleId,
    ) -> sclc::TypeEnv<'a> {
        let env = bindings.iter().fold(
            sclc::TypeEnv::new().with_module_id(module_id),
            |env, (name, (ty, _))| env.with_local(name.as_str(), sclc::Span::default(), ty.clone()),
        );
        type_defs.iter().fold(env, |env, (name, ty)| {
            env.with_type_level(name.clone(), ty.clone())
        })
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
    let eval = sclc::Eval::new::<FsSource>(effects_tx, String::from("cli/repl"));
    let mut repl = Repl::new(eval);
    let helper = ReplHelper::new();
    let mut editor = Editor::new()?;
    editor.set_helper(Some(helper));

    loop {
        match editor.readline("scl> ") {
            Ok(line) => {
                editor.add_history_entry(&line)?;

                let helper = editor.helper().expect("helper is set");
                if let Err(e) = repl.process(&line, helper).await {
                    println!("{e}");
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }

    drop(repl);
    effects_task.await?;

    Ok(())
}
