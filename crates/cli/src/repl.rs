use std::borrow::Cow;
use std::path::{Path, PathBuf};

use rustyline::completion::{self, Candidate};
use rustyline::error::ReadlineError;
use rustyline::highlight;
use rustyline::hint;
use rustyline::validate;
use rustyline::{Context, Editor, Helper};
use sclc::{Diag, Lexer, Token};

use crate::fs_source::FsSource;
use crate::output::{report_diagnostics, spawn_effect_printer};

struct ReplHelper;

struct CompletionEntry(String);

impl Candidate for CompletionEntry {
    fn display(&self) -> &str {
        &self.0
    }

    fn replacement(&self) -> &str {
        &self.0
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
        // Completions require access to the ReplState, which is not available
        // from the rustyline helper. Completion is handled in the main loop
        // via the ReplState's type_env method. For now, return empty.
        let _ = (line, pos);
        Ok((0, Vec::new()))
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
                Token::QuestionQuestion => emit!("??"),

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

fn report_repl_error(err: &sclc::ReplError) {
    match err {
        sclc::ReplError::Diagnostics(diags) => {
            for d in diags.iter() {
                let (module_id, span) = d.locate();
                println!("[{:?}] {module_id}:{span}: {d}", d.level());
            }
        }
        sclc::ReplError::TypeCheck(e) => println!("{e}"),
        sclc::ReplError::Eval(e) => println!("{e}"),
    }
}

async fn process_line(
    state: &mut sclc::ReplState<FsSource>,
    root: &Path,
    line: &str,
) -> anyhow::Result<()> {
    let module_id = state.next_line_module_id();
    let Some(repl_line) =
        report_diagnostics(sclc::parse_repl_line(line, &module_id)).and_then(|line| line)
    else {
        return Ok(());
    };

    let Some(statement) = &repl_line.statement else {
        return Ok(());
    };

    // Handle imports separately — they mutate the program
    if let sclc::ModStmt::Import(import_stmt) = statement {
        return process_import(state, root, import_stmt).await;
    }

    match state.process_statement(statement, &module_id) {
        Ok(sclc::ReplOutcome::Binding { name, ty }) => {
            println!("{name} : {ty}");
        }
        Ok(sclc::ReplOutcome::Value { value }) => {
            println!("{}", value.value);
        }
        Ok(sclc::ReplOutcome::TypeDef { .. }) => {}
        Err(err) => report_repl_error(&err),
    }

    Ok(())
}

async fn process_import(
    state: &mut sclc::ReplState<FsSource>,
    root: &Path,
    import_stmt: &sclc::Loc<sclc::ImportStmt>,
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

    // Open the user package if not already loaded and resolve imports
    let package_id = state
        .program()
        .self_package_id()
        .cloned()
        .unwrap_or_default();
    let source = FsSource {
        root: root.to_path_buf(),
        package_id,
    };
    state.program_mut().open_package(source).await;
    let Some(file_mod) = state
        .program_mut()
        .resolve_import(&import_path)
        .await?
        .cloned()
    else {
        let vars = &import_stmt.as_ref().vars;
        let path_span = sclc::Span::new(
            vars.first()
                .expect("import has at least one segment")
                .span()
                .start(),
            vars.last()
                .expect("import has at least one segment")
                .span()
                .end(),
        );
        let diag = sclc::InvalidImport {
            source_module_id: ["Repl"].into(),
            import_path,
            path_span,
        };
        let (module_id, span) = diag.locate();
        println!("[{:?}] {module_id}:{span}: {diag}", diag.level());
        return Ok(());
    };

    // Resolve transitive imports
    let _ = state.program_mut().resolve_imports().await;

    let checker = sclc::TypeChecker::new(state.program());
    let type_env = sclc::TypeEnv::new().with_module_id(&import_path);
    let diagnosed_ty = checker.check_file_mod(&type_env, &file_mod)?;
    let Some(ty) = report_diagnostics(diagnosed_ty) else {
        return Ok(());
    };

    let eval = state.make_eval();
    let value = state.program().evaluate(&import_path, &eval)?.into_inner();

    let type_exports = checker
        .type_level_exports(&type_env, &file_mod)
        .into_inner();

    state.register_import(alias, ty, value, type_exports);

    Ok(())
}

pub async fn run_repl(root: PathBuf, package: String) -> anyhow::Result<()> {
    let package_id = package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<sclc::ModuleId>();
    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let effects_task = spawn_effect_printer(effects_rx);

    // Create a persistent program for the REPL session
    let mut program = sclc::Program::<FsSource>::new();
    let source = FsSource {
        root: root.clone(),
        package_id: package_id.clone(),
    };
    program.open_package(source).await;

    let mut state = sclc::ReplState::new(program, effects_tx, package_id.to_string());
    let mut editor = Editor::new()?;
    editor.set_helper(Some(ReplHelper));

    loop {
        match editor.readline("scl> ") {
            Ok(line) => {
                editor.add_history_entry(&line)?;

                if let Err(e) = process_line(&mut state, &root, &line).await {
                    println!("{e}");
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }

    drop(state);
    effects_task.await?;

    Ok(())
}
