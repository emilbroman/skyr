use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use rustyline::completion::{self, Candidate};
use rustyline::error::ReadlineError;
use rustyline::highlight;
use rustyline::hint;
use rustyline::validate;
use rustyline::{Context, Editor, Helper};
use sclc::{Diag, Lexer, Token};

use crate::fs_source::FsSource;
use crate::output::{report_diagnostics, spawn_effect_printer};

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

        let candidates = info
            .completion_candidates
            .iter()
            .map(|c| match c {
                sclc::CompletionCandidate::Var(name) => CompletionEntry(name.clone()),
                sclc::CompletionCandidate::Member(member) => CompletionEntry(member.name.clone()),
            })
            .collect();

        Ok((start, candidates))
    }
}

impl hint::Hinter for ReplHelper {
    type Hint = String;
}

impl highlight::Highlighter for ReplHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let mut result = String::with_capacity(line.len() + 64);
        let mut source_bytes_covered: usize = 0;

        macro_rules! emit {
            ($text:expr) => {{
                let t = $text;
                source_bytes_covered += t.len();
                result.push_str(t);
            }};
            ($text:expr, $color:expr) => {{
                let t = $text;
                source_bytes_covered += t.len();
                result.push_str($color);
                result.push_str(t);
                result.push_str("\x1b[0m");
            }};
        }

        for token in Lexer::new(line) {
            match *token.as_ref() {
                // Keywords
                Token::ImportKeyword => emit!("import", "\x1b[35m"),
                Token::LetKeyword => emit!("let", "\x1b[35m"),
                Token::FnKeyword => emit!("fn", "\x1b[35m"),
                Token::ExportKeyword => emit!("export", "\x1b[35m"),
                Token::ExternKeyword => emit!("extern", "\x1b[35m"),
                Token::IfKeyword => emit!("if", "\x1b[35m"),
                Token::ElseKeyword => emit!("else", "\x1b[35m"),
                Token::ForKeyword => emit!("for", "\x1b[35m"),
                Token::InKeyword => emit!("in", "\x1b[35m"),
                Token::TypeKeyword => emit!("type", "\x1b[35m"),
                Token::ExceptionKeyword => emit!("exception", "\x1b[35m"),
                Token::RaiseKeyword => emit!("raise", "\x1b[35m"),
                Token::TryKeyword => emit!("try", "\x1b[35m"),
                Token::CatchKeyword => emit!("catch", "\x1b[35m"),
                Token::AsKeyword => emit!("as", "\x1b[35m"),

                // Literal keywords
                Token::NilKeyword => emit!("nil", "\x1b[33m"),
                Token::TrueKeyword => emit!("true", "\x1b[33m"),
                Token::FalseKeyword => emit!("false", "\x1b[33m"),

                // Numbers
                Token::Int(s) | Token::Float(s) => emit!(s, "\x1b[33m"),

                // Strings
                Token::StrSimple(s) => {
                    source_bytes_covered += 1 + s.len() + 1; // opening + content + closing quote
                    result.push_str("\x1b[32m\"");
                    result.push_str(s);
                    result.push_str("\"\x1b[0m");
                }
                Token::StrBegin(s) => {
                    source_bytes_covered += 1 + s.len() + 1; // opening quote + content + {
                    result.push_str("\x1b[32m\"");
                    result.push_str(s);
                    result.push_str("\x1b[0m{");
                }
                Token::StrCont(s) => {
                    source_bytes_covered += 1 + s.len() + 1; // } + content + {
                    result.push_str("}\x1b[32m");
                    result.push_str(s);
                    result.push_str("\x1b[0m{");
                }
                Token::StrEnd(s) => {
                    source_bytes_covered += 1 + s.len() + 1; // } + content + closing quote
                    result.push_str("}\x1b[32m");
                    result.push_str(s);
                    result.push_str("\"\x1b[0m");
                }

                // Comments
                Token::Comment(s) | Token::DocComment(s) => emit!(s, "\x1b[90m"),

                // Punctuation and operators (no color)
                Token::OpenCurly => emit!("{"),
                Token::CloseCurly => emit!("}"),
                Token::OpenParen => emit!("("),
                Token::CloseParen => emit!(")"),
                Token::OpenSquare => emit!("["),
                Token::CloseSquare => emit!("]"),
                Token::Hash => emit!("#"),
                Token::Colon => emit!(":"),
                Token::Comma => emit!(","),
                Token::Dot => emit!("."),
                Token::Equals => emit!("="),
                Token::EqEq => emit!("=="),
                Token::Semicolon => emit!(";"),
                Token::Slash => emit!("/"),
                Token::Plus => emit!("+"),
                Token::Minus => emit!("-"),
                Token::Star => emit!("*"),
                Token::BangEq => emit!("!="),
                Token::Less => emit!("<"),
                Token::LessColon => emit!("<:"),
                Token::LessEq => emit!("<="),
                Token::Greater => emit!(">"),
                Token::GreaterEq => emit!(">="),
                Token::AndAnd => emit!("&&"),
                Token::OrOr => emit!("||"),
                Token::QuestionMark => emit!("?"),

                // Identifiers, whitespace, unknown
                Token::Symbol(s)
                | Token::Whitepace(s)
                | Token::Unknown(s)
                | Token::Cursor { content: s, .. } => emit!(s),
            }
        }

        // The lexer consumes characters inside unterminated strings/comments
        // without emitting them as token content. Recover the missing tail.
        if source_bytes_covered < line.len() {
            let tail = &line[source_bytes_covered..];
            result.push_str("\x1b[32m");
            result.push_str(tail);
            result.push_str("\x1b[0m");
        }

        Cow::Owned(result)
    }

    fn highlight_char(&self, _line: &str, _pos: usize, _forced: bool) -> bool {
        true
    }
}
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
            report_diagnostics(sclc::parse_repl_line(line, &module_id)).and_then(|line| line)
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
                let Some(ty) = report_diagnostics(diagnosed) else {
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
                let Some(_) = report_diagnostics(diagnosed) else {
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
                let Some(ty) = report_diagnostics(diagnosed) else {
                    return Ok(());
                };

                let eval_env = Self::eval_env(&state.bindings, &module_id);
                let value = self.eval.eval_expr(&eval_env, &let_bind.expr)?;
                println!("{} : {}", let_bind.var.name, ty);
                Some((let_bind.var.name.clone(), (ty, value)))
            }
            sclc::ModStmt::TypeDef(type_def) | sclc::ModStmt::ExportTypeDef(type_def) => {
                let checker = sclc::TypeChecker::new(&program);
                let Some(ty) = report_diagnostics(checker.resolve_type_def(&type_env, type_def))
                else {
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
        let Some(ty) = report_diagnostics(diagnosed_ty) else {
            return Ok(());
        };

        let eval_env = sclc::EvalEnv::new().with_module_id(&import_path);
        let value = self.eval.eval_file_mod(&eval_env, &file_mod)?;

        // Extract type-level exports so that imported types are available in subsequent lines.
        let type_exports = checker
            .type_level_exports(&type_env, &file_mod)
            .into_inner();

        let mut state = helper.state.borrow_mut();
        state.bindings.insert(alias.clone(), (ty, value));
        if type_exports.iter().next().is_some() {
            state
                .type_defs
                .insert(alias, sclc::Type::Record(type_exports));
        }

        Ok(())
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
            env.with_type_level(name.clone(), ty.clone(), None)
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
    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let effects_task = spawn_effect_printer(effects_rx);
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
