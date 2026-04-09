use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use rustyline::completion::{self, Candidate};
use rustyline::error::ReadlineError;
use rustyline::highlight;
use rustyline::hint;
use rustyline::validate;
use rustyline::{Context, Editor, Helper};
use sclc::{Lexer, Token};

use crate::output::spawn_effect_printer;

struct ReplHelper {
    /// Snapshot of the REPL state used for completions.
    /// Updated before each `readline()` call.
    state: RefCell<Option<sclc::Repl>>,
    /// Filesystem root for path completions.
    root: PathBuf,
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

impl completion::Completer for ReplHelper {
    type Candidate = CompletionEntry;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        // Check if the cursor is inside a path expression and handle it
        // with direct filesystem access (no async cache needed).
        if let Some(result) = self.complete_path(line, pos) {
            return Ok(result);
        }

        let state_opt = self.state.borrow();
        let Some(state) = state_opt.as_ref() else {
            return Ok((0, Vec::new()));
        };

        // Build a cursor at the given position (1-based line/column).
        let position = sclc::Position::new(1, pos as u32 + 1);
        let cursor = sclc::Cursor::new(position);

        // Use a peek module ID (don't increment the line counter).
        let module_id = sclc::ModuleId::new(
            state
                .program()
                .self_package_id()
                .cloned()
                .unwrap_or_default(),
            vec!["ReplCompletion".to_string()],
        );

        let diagnosed = sclc::parse_repl_line_with_cursor(line, &module_id, Some(cursor.clone()));
        let repl_line = match diagnosed.into_inner() {
            Some(rl) => rl,
            None => return Ok((0, Vec::new())),
        };
        let Some(statement) = &repl_line.statement else {
            return Ok((0, Vec::new()));
        };

        // Type-check the statement to populate completion candidates.
        let type_env = state.type_env(&module_id);
        let checker = sclc::TypeChecker::new(state.unit());
        let _ = checker.check_stmt(&type_env, statement);

        // Extract candidates from the cursor.
        let info = cursor.info();
        let info = info.lock().unwrap();

        // Determine the prefix the user has typed so far for replacement range.
        // Walk backwards from `pos` to find the start of the current token.
        let before = &line[..pos];
        let token_start = before
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .map(|i| i + 1)
            .unwrap_or(0);

        let candidates: Vec<CompletionEntry> = info
            .completion_candidates
            .iter()
            .map(|c| match c {
                sclc::CompletionCandidate::Var(name) => CompletionEntry(name.clone()),
                sclc::CompletionCandidate::Member(m) => CompletionEntry(m.name.clone()),
                sclc::CompletionCandidate::Module(name) => CompletionEntry(name.clone()),
                sclc::CompletionCandidate::ModuleDir(name) => CompletionEntry(format!("{name}/")),
                sclc::CompletionCandidate::PathFile(name) => CompletionEntry(name.clone()),
                sclc::CompletionCandidate::PathDir(name) => CompletionEntry(format!("{name}/")),
            })
            .collect();

        Ok((token_start, candidates))
    }
}

impl ReplHelper {
    /// Try to complete a filesystem path expression. Returns `None` if the
    /// cursor isn't inside a path expression.
    fn complete_path(&self, line: &str, pos: usize) -> Option<(usize, Vec<CompletionEntry>)> {
        let before = &line[..pos];

        // Find the start of the path expression by scanning backwards for
        // the `./` or `../` or leading `/` that begins a path literal.
        // Path tokens are: alphanumeric, `.`, `_`, `-`, `/`
        let path_char = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '/');
        let token_start = before
            .rfind(|c: char| !path_char(c))
            .map(|i| i + 1)
            .unwrap_or(0);
        let path_text = &before[token_start..];

        // Only handle path expressions (starting with `./ `, `../ `, or `/`).
        if !path_text.starts_with("./")
            && !path_text.starts_with("../")
            && !path_text.starts_with('/')
        {
            return None;
        }

        // Split into directory prefix and filename prefix at the last `/`.
        let last_slash = path_text.rfind('/')?;
        let dir_part = &path_text[..=last_slash]; // includes trailing /
        let name_prefix = &path_text[last_slash + 1..];

        // Resolve the directory to a filesystem path.
        let fs_dir = self.root.join(
            dir_part
                .strip_prefix("./")
                .or_else(|| dir_part.strip_prefix('/'))
                .unwrap_or(dir_part),
        );

        // Read the directory synchronously.
        let read_dir = std::fs::read_dir(&fs_dir).ok()?;

        let completion_start = token_start + last_slash + 1;
        let mut candidates = Vec::new();
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }
            if !name.starts_with(name_prefix) {
                continue;
            }
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if is_dir {
                candidates.push(CompletionEntry(format!("{name}/")));
            } else {
                candidates.push(CompletionEntry(name));
            }
        }
        candidates.sort_by(|a, b| a.0.cmp(&b.0));

        Some((completion_start, candidates))
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

        // Collect tokens into a Vec for lookahead-based path detection.
        let tokens: Vec<_> = Lexer::new(line).collect();

        // Pre-compute which token indices are part of a path expression.
        // A path starts with:
        //   - Dot Slash (relative: ./...)
        //   - Dot Dot Slash (relative: ../...)
        //   - Slash Symbol (absolute: /foo...)
        // and continues with Slash, Symbol, StrSimple, Dot sequences.
        let mut in_path = vec![false; tokens.len()];
        let mut i = 0;
        while i < tokens.len() {
            let starts_path = if matches!(*tokens[i].as_ref(), Token::Dot) {
                if i + 1 < tokens.len() && matches!(*tokens[i + 1].as_ref(), Token::Slash) {
                    // ./ — check there's a segment after
                    i + 2 < tokens.len()
                        && matches!(
                            *tokens[i + 2].as_ref(),
                            Token::Symbol(_) | Token::StrSimple(_)
                        )
                } else if i + 2 < tokens.len()
                    && matches!(*tokens[i + 1].as_ref(), Token::Dot)
                    && matches!(*tokens[i + 2].as_ref(), Token::Slash)
                {
                    // ../ — check there's a segment after
                    i + 3 < tokens.len()
                        && matches!(
                            *tokens[i + 3].as_ref(),
                            Token::Symbol(_) | Token::StrSimple(_)
                        )
                } else {
                    false
                }
            } else if matches!(*tokens[i].as_ref(), Token::Slash) {
                // /segment — absolute path
                i + 1 < tokens.len()
                    && matches!(
                        *tokens[i + 1].as_ref(),
                        Token::Symbol(_) | Token::StrSimple(_)
                    )
            } else {
                false
            };

            if starts_path {
                // Mark all tokens in this path expression.
                while i < tokens.len() {
                    match *tokens[i].as_ref() {
                        Token::Dot | Token::Slash | Token::Symbol(_) | Token::StrSimple(_) => {
                            in_path[i] = true;
                            i += 1;
                        }
                        _ => break,
                    }
                }
            } else {
                i += 1;
            }
        }

        for (idx, token) in tokens.iter().enumerate() {
            if in_path[idx] {
                // Emit path tokens in green (same as strings).
                match *token.as_ref() {
                    Token::Dot => emit!(".", "\x1b[32m"),
                    Token::Slash => emit!("/", "\x1b[32m"),
                    Token::Symbol(s) => emit!(s, "\x1b[32m"),
                    Token::StrSimple(s) => {
                        source_bytes_covered += 1 + s.len() + 1;
                        result.push_str("\x1b[32m\"");
                        result.push_str(s);
                        result.push_str("\"\x1b[0m");
                    }
                    _ => unreachable!(),
                }
                continue;
            }

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
        sclc::ReplError::Resolve(e) => println!("{e}"),
        sclc::ReplError::ResolveImport(e) => println!("{e}"),
    }
}

async fn process_line(state: &mut sclc::Repl, root: &Path, line: &str) -> anyhow::Result<()> {
    let package_id = state
        .program()
        .self_package_id()
        .cloned()
        .unwrap_or_default();
    state.replace_user_source(sclc::FsSource {
        root: root.to_path_buf(),
        package_id,
    });

    match state.process(line.to_owned()).await {
        Ok(Some(sclc::ReplOutcome::Binding { name, ty })) => {
            println!("{name} : {ty}");
        }
        Ok(Some(sclc::ReplOutcome::Value { value })) => {
            println!("{}", value.value);
        }
        Ok(Some(sclc::ReplOutcome::TypeDef { .. })) => {}
        Ok(Some(sclc::ReplOutcome::Import { module_id })) => {
            println!("import {module_id}");
        }
        Ok(None) => {}
        Err(err) => report_repl_error(&err),
    }

    Ok(())
}

pub async fn run_repl(root: PathBuf, package: String) -> anyhow::Result<()> {
    let package_id = package
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect::<sclc::PackageId>();
    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let effects_task = spawn_effect_printer(effects_rx);

    // Create a persistent program for the REPL session
    let mut program = sclc::Program::new();
    let source = sclc::FsSource {
        root: root.clone(),
        package_id: package_id.clone(),
    };
    program.open_package(source).await;
    // Preload root directory listing so path validation works for REPL lines
    program.preload_path_dirs([PathBuf::new()]).await;

    let mut state = sclc::Repl::new(program, effects_tx, package_id.to_string());
    let mut editor = Editor::new()?;
    editor.set_helper(Some(ReplHelper {
        state: RefCell::new(None),
        root: root.clone(),
    }));

    loop {
        // Snapshot the state into the helper so completions work during readline.
        if let Some(helper) = editor.helper() {
            *helper.state.borrow_mut() = Some(state.clone());
        }

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

    // Clear the helper's snapshot so its cloned effects_tx sender is dropped.
    if let Some(helper) = editor.helper() {
        *helper.state.borrow_mut() = None;
    }
    drop(state);
    effects_task.await?;

    Ok(())
}
