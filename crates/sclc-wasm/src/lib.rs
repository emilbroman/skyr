use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use wasm_bindgen::prelude::*;

/// In-memory source repository holding multiple `.scl` files.
struct MemSourceRepo {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl sclc::SourceRepo for MemSourceRepo {
    type Err = Infallible;

    fn package_id(&self) -> sclc::ModuleId {
        ["Playground"].into_iter().map(String::from).collect()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        Ok(self.files.get(path).cloned())
    }

    async fn list_children(&self, path: &Path) -> Result<Vec<sclc::ChildEntry>, Self::Err> {
        let mut modules = HashSet::new();
        let mut dirs = HashSet::new();

        for file_path in self.files.keys() {
            let relative = if path == Path::new("") {
                Some(file_path.as_path())
            } else {
                file_path.strip_prefix(path).ok()
            };

            let Some(relative) = relative else {
                continue;
            };

            let components: Vec<_> = relative.components().collect();
            match components.len() {
                1 => {
                    if let Some(stem) = relative.file_stem()
                        && relative.extension().and_then(|e| e.to_str()) == Some("scl")
                    {
                        modules.insert(stem.to_string_lossy().into_owned());
                    }
                }
                n if n > 1 => {
                    if let Some(dir) = components.first() {
                        dirs.insert(dir.as_os_str().to_string_lossy().into_owned());
                    }
                }
                _ => {}
            }
        }

        let mut entries: Vec<sclc::ChildEntry> = dirs
            .into_iter()
            .map(sclc::ChildEntry::Directory)
            .chain(modules.into_iter().map(sclc::ChildEntry::Module))
            .collect();
        entries.sort();
        Ok(entries)
    }
}

fn make_repo(files_json: &str) -> MemSourceRepo {
    let file_map: HashMap<String, String> = serde_json::from_str(files_json).unwrap_or_default();
    MemSourceRepo {
        files: file_map
            .into_iter()
            .map(|(name, content)| (PathBuf::from(name), content.into_bytes()))
            .collect(),
    }
}

fn parse_files_json(files_json: &str) -> HashMap<String, String> {
    serde_json::from_str(files_json).unwrap_or_default()
}

/// Derive module ID from a file path relative to the package root.
/// e.g. "models/User.scl" -> ["Playground", "models", "User"]
fn module_id_for_file(file: &str) -> sclc::ModuleId {
    let path = Path::new(file);
    let mut segments: Vec<String> = vec!["Playground".to_string()];
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            segments.push(component.as_os_str().to_string_lossy().into_owned());
        }
    }
    if let Some(stem) = path.file_stem() {
        segments.push(stem.to_string_lossy().into_owned());
    }
    segments.into_iter().collect()
}

/// Convert a module ID back to a file path relative to the package root.
/// e.g. ["Playground", "models", "User"] -> "models/User.scl"
fn file_for_module_id(module_id: &sclc::ModuleId) -> Option<String> {
    let package_id: sclc::ModuleId = ["Playground"].into_iter().map(String::from).collect();
    let segments = module_id.suffix_after(&package_id)?;
    if segments.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    for s in segments {
        path.push(s);
    }
    path.set_extension("scl");
    Some(path.to_string_lossy().into_owned())
}

/// Load a program from multiple files (compile + type check), returning diagnostics.
async fn load_and_compile(files_json: &str) -> (sclc::DiagList, sclc::Program<MemSourceRepo>) {
    let repo = make_repo(files_json);
    let file_map = parse_files_json(files_json);
    let mut diags = sclc::DiagList::new();

    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;

    for name in file_map.keys() {
        if name.ends_with(".scl") {
            let _ = package.open(name).await;
        }
    }

    if let Ok(diagnosed) = program.resolve_imports().await {
        diagnosed.unpack(&mut diags);
    }

    if let Ok(diagnosed) = program.check_types() {
        diagnosed.unpack(&mut diags);
    }

    (diags, program)
}

#[derive(Serialize)]
struct DiagnosticInfo {
    file: String,
    line: u32,
    character: u32,
    end_line: u32,
    end_character: u32,
    message: String,
    severity: &'static str,
}

/// Analyze all files and return diagnostics as JSON.
#[wasm_bindgen]
pub async fn analyze(files_json: &str) -> String {
    let (diags, _) = load_and_compile(files_json).await;

    let package_id: sclc::ModuleId = ["Playground"].into_iter().map(String::from).collect();

    let result: Vec<DiagnosticInfo> = diags
        .iter()
        .filter(|d| {
            let (module_id, _) = d.locate();
            module_id.starts_with(&package_id)
        })
        .map(|d| {
            let (module_id, span) = d.locate();
            let level = d.level();
            let file = file_for_module_id(&module_id).unwrap_or_default();
            DiagnosticInfo {
                file,
                line: span.start().line().saturating_sub(1),
                character: span.start().character().saturating_sub(1),
                end_line: span.end().line().saturating_sub(1),
                end_character: span.end().character().saturating_sub(1),
                message: d.to_string(),
                severity: match level {
                    sclc::DiagLevel::Error => "error",
                    sclc::DiagLevel::Warning => "warning",
                },
            }
        })
        .collect();

    serde_json::to_string(&result).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Serialize)]
struct HoverInfo {
    #[serde(rename = "type")]
    ty: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// Get hover information (type + description) at a position in a specific file.
#[wasm_bindgen]
pub async fn hover(files_json: &str, file: &str, line: u32, col: u32) -> Option<String> {
    let file_map = parse_files_json(files_json);
    let repo = make_repo(files_json);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;

    for name in file_map.keys() {
        if name.ends_with(".scl") {
            let _ = package.open(name).await;
        }
    }
    let _ = program.resolve_imports().await;

    let module_id = module_id_for_file(file);
    let source = file_map.get(file)?;
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&program, source, &module_id, position);

    let info = cursor_info.lock().unwrap();
    let ty_str = match (&info.identifier, &info.ty) {
        (Some(sclc::CursorIdentifier::Let(name)), Some(ty)) => Some(format!("let {name}: {ty}")),
        (Some(sclc::CursorIdentifier::Type(name)), Some(ty)) => Some(format!("type {name} {ty}")),
        (None, Some(ty)) => Some(ty.to_string()),
        _ => None,
    };
    ty_str.map(|ty| {
        serde_json::to_string(&HoverInfo {
            ty,
            description: info.description.clone(),
        })
        .unwrap()
    })
}

#[derive(Serialize)]
struct CompletionItem {
    label: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// Get completions at a position in a specific file.
#[wasm_bindgen]
pub async fn completions(files_json: &str, file: &str, line: u32, col: u32) -> String {
    let file_map = parse_files_json(files_json);
    let repo = make_repo(files_json);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;

    for name in file_map.keys() {
        if name.ends_with(".scl") {
            let _ = package.open(name).await;
        }
    }
    let _ = program.resolve_imports().await;

    let module_id = module_id_for_file(file);
    let Some(source) = file_map.get(file) else {
        return "[]".to_string();
    };
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&program, source, &module_id, position);

    let info = cursor_info.lock().unwrap();
    let items: Vec<CompletionItem> = info
        .completion_candidates
        .iter()
        .map(|c| match c {
            sclc::CompletionCandidate::Var(name) => CompletionItem {
                label: name.clone(),
                kind: "variable",
                detail: None,
                description: None,
            },
            sclc::CompletionCandidate::Member(member) => CompletionItem {
                label: member.name.clone(),
                kind: "field",
                detail: member
                    .ty
                    .as_ref()
                    .map(|ty| format!("let {}: {ty}", &member.name)),
                description: member.description.clone(),
            },
            sclc::CompletionCandidate::Module(name) => CompletionItem {
                label: name.clone(),
                kind: "module",
                detail: None,
                description: None,
            },
            sclc::CompletionCandidate::ModuleDir(name) => CompletionItem {
                label: name.clone(),
                kind: "folder",
                detail: None,
                description: None,
            },
        })
        .collect();

    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Serialize)]
struct LocationInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    line: u32,
    character: u32,
    end_line: u32,
    end_character: u32,
}

/// Get go-to-definition location at a position in a specific file.
#[wasm_bindgen]
pub async fn goto_definition(files_json: &str, file: &str, line: u32, col: u32) -> Option<String> {
    let file_map = parse_files_json(files_json);
    let repo = make_repo(files_json);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;

    for name in file_map.keys() {
        if name.ends_with(".scl") {
            let _ = package.open(name).await;
        }
    }
    let _ = program.resolve_imports().await;

    let module_id = module_id_for_file(file);
    let source = file_map.get(file)?;
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&program, source, &module_id, position);

    let info = cursor_info.lock().unwrap();
    info.declaration.map(|span| {
        serde_json::to_string(&LocationInfo {
            file: None,
            line: span.start().line().saturating_sub(1),
            character: span.start().character().saturating_sub(1),
            end_line: span.end().line().saturating_sub(1),
            end_character: span.end().character().saturating_sub(1),
        })
        .unwrap()
    })
}

/// Format source code (single file).
#[wasm_bindgen]
pub fn format(source: &str) -> Option<String> {
    let module_id: sclc::ModuleId = ["Playground", "Main"]
        .into_iter()
        .map(String::from)
        .collect();
    let diagnosed = sclc::parse_file_mod(source, &module_id);
    let file_mod = diagnosed.into_inner();
    let formatted = sclc::Formatter::format(source, &file_mod);
    if formatted == source {
        None
    } else {
        Some(formatted)
    }
}

fn query_cursor(
    program: &sclc::Program<MemSourceRepo>,
    source: &str,
    module_id: &sclc::ModuleId,
    position: sclc::Position,
) -> Arc<Mutex<sclc::CursorInfo>> {
    let cursor = sclc::Cursor::new(position);
    let cursor_info = cursor.info();

    let diagnosed = sclc::parse_file_mod_with_cursor(source, module_id, Some(cursor.clone()));
    let file_mod = diagnosed.into_inner();

    let type_env = sclc::TypeEnv::new()
        .with_module_id(module_id)
        .with_cursor(cursor);
    let checker = sclc::TypeChecker::new(program);
    let _ = checker.check_file_mod(&type_env, &file_mod);

    cursor_info
}

// ---------------------------------------------------------------------------
// REPL support
// ---------------------------------------------------------------------------

struct ReplState {
    line_number: usize,
    eval: sclc::Eval,
    bindings: HashMap<String, (sclc::Type, sclc::TrackedValue)>,
    type_defs: HashMap<String, sclc::Type>,
    effects_rx: tokio::sync::mpsc::UnboundedReceiver<sclc::Effect>,
}

thread_local! {
    static REPL_STATE: RefCell<Option<ReplState>> = const { RefCell::new(None) };
}

/// Initialize a fresh REPL session.
#[wasm_bindgen]
pub fn repl_init() {
    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let eval = sclc::Eval::new::<MemSourceRepo>(effects_tx, "Playground".to_string());
    REPL_STATE.with(|cell| {
        *cell.borrow_mut() = Some(ReplState {
            line_number: 0,
            eval,
            bindings: HashMap::new(),
            type_defs: HashMap::new(),
            effects_rx,
        });
    });
}

/// Reset the REPL session (re-initializes).
#[wasm_bindgen]
pub fn repl_reset() {
    repl_init();
}

#[derive(Serialize)]
struct ReplResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    effects: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn format_effect(effect: &sclc::Effect) -> String {
    match effect {
        sclc::Effect::CreateResource { id, .. } => {
            format!("CreateResource {}:{}", id.typ, id.name)
        }
        sclc::Effect::UpdateResource { id, .. } => {
            format!("UpdateResource {}:{}", id.typ, id.name)
        }
        sclc::Effect::TouchResource { id, .. } => {
            format!("TouchResource {}:{}", id.typ, id.name)
        }
    }
}

fn drain_effects(rx: &mut tokio::sync::mpsc::UnboundedReceiver<sclc::Effect>) -> Vec<String> {
    let mut effects = Vec::new();
    while let Ok(effect) = rx.try_recv() {
        effects.push(format_effect(&effect));
    }
    effects
}

fn collect_diagnostics(diags: &sclc::DiagList) -> Vec<String> {
    diags
        .iter()
        .map(|d| {
            let (module_id, span) = d.locate();
            format!("[{:?}] {module_id}:{span}: {d}", d.level())
        })
        .collect()
}

/// Evaluate a REPL line. Returns JSON with { output?, effects?, error? }.
#[wasm_bindgen]
pub async fn repl_eval(files_json: &str, line: &str) -> String {
    // Take state out of thread-local
    let state = REPL_STATE.with(|cell| cell.borrow_mut().take());
    let Some(mut state) = state else {
        return r#"{"error":"REPL not initialized. Call repl_init() first."}"#.to_string();
    };

    let result = repl_process(&mut state, files_json, line).await;

    // Put state back
    REPL_STATE.with(|cell| {
        *cell.borrow_mut() = Some(state);
    });

    serde_json::to_string(&result)
        .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string())
}

async fn repl_process(state: &mut ReplState, files_json: &str, line: &str) -> ReplResult {
    state.line_number += 1;

    let module_id: sclc::ModuleId = [format!("Repl{}", state.line_number)].into();
    let parsed = sclc::parse_repl_line(line, &module_id);
    let mut diags = sclc::DiagList::new();
    let repl_line = parsed.unpack(&mut diags);

    if !diags.iter().all(|d| d.level() != sclc::DiagLevel::Error) {
        let errors = collect_diagnostics(&diags);
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: Some(errors.join("\n")),
        };
    }

    let Some(repl_line) = repl_line else {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: None,
        };
    };

    let Some(statement) = &repl_line.statement else {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: None,
        };
    };

    // Handle imports separately
    if let sclc::ModStmt::Import(import_stmt) = statement {
        return repl_process_import(state, files_json, import_stmt).await;
    }

    let program = sclc::Program::<MemSourceRepo>::new();
    let type_env = repl_type_env(&state.bindings, &state.type_defs, &module_id);

    match statement {
        sclc::ModStmt::Import(_) => unreachable!(),
        sclc::ModStmt::Let(let_bind) | sclc::ModStmt::Export(let_bind) => {
            let checker = sclc::TypeChecker::new(&program);
            let type_result = checker.check_global_let_bind(&type_env, let_bind);
            let Ok(diagnosed) = type_result else {
                return ReplResult {
                    output: None,
                    effects: Vec::new(),
                    error: Some(format!("{}", type_result.unwrap_err())),
                };
            };
            let ty = diagnosed.unpack(&mut diags);
            if diags.iter().any(|d| d.level() == sclc::DiagLevel::Error) {
                return ReplResult {
                    output: None,
                    effects: Vec::new(),
                    error: Some(collect_diagnostics(&diags).join("\n")),
                };
            }

            let eval_env = repl_eval_env(&state.bindings, &module_id);
            match state.eval.eval_expr(&eval_env, &let_bind.expr) {
                Ok(value) => {
                    let output = format!("{} : {ty}", let_bind.var.name);
                    state
                        .bindings
                        .insert(let_bind.var.name.clone(), (ty, value));
                    let effects = drain_effects(&mut state.effects_rx);
                    ReplResult {
                        output: Some(output),
                        effects,
                        error: None,
                    }
                }
                Err(e) => ReplResult {
                    output: None,
                    effects: drain_effects(&mut state.effects_rx),
                    error: Some(e.to_string()),
                },
            }
        }
        sclc::ModStmt::Expr(expr) => {
            let checker = sclc::TypeChecker::new(&program);
            let type_result = checker.check_stmt(&type_env, statement);
            let Ok(diagnosed) = type_result else {
                return ReplResult {
                    output: None,
                    effects: Vec::new(),
                    error: Some(format!("{}", type_result.unwrap_err())),
                };
            };
            diagnosed.unpack(&mut diags);
            if diags.iter().any(|d| d.level() == sclc::DiagLevel::Error) {
                return ReplResult {
                    output: None,
                    effects: Vec::new(),
                    error: Some(collect_diagnostics(&diags).join("\n")),
                };
            }

            let eval_env = repl_eval_env(&state.bindings, &module_id);
            match state.eval.eval_expr(&eval_env, expr) {
                Ok(value) => {
                    let effects = drain_effects(&mut state.effects_rx);
                    ReplResult {
                        output: Some(format!("{}", value.value)),
                        effects,
                        error: None,
                    }
                }
                Err(e) => ReplResult {
                    output: None,
                    effects: drain_effects(&mut state.effects_rx),
                    error: Some(e.to_string()),
                },
            }
        }
        sclc::ModStmt::TypeDef(type_def) | sclc::ModStmt::ExportTypeDef(type_def) => {
            let checker = sclc::TypeChecker::new(&program);
            let diagnosed = checker.resolve_type_def(&type_env, type_def);
            let ty = diagnosed.unpack(&mut diags);
            if diags.iter().any(|d| d.level() == sclc::DiagLevel::Error) {
                return ReplResult {
                    output: None,
                    effects: Vec::new(),
                    error: Some(collect_diagnostics(&diags).join("\n")),
                };
            }
            state.type_defs.insert(type_def.var.name.clone(), ty);
            ReplResult {
                output: Some(format!("type {}", type_def.var.name)),
                effects: Vec::new(),
                error: None,
            }
        }
    }
}

async fn repl_process_import(
    state: &mut ReplState,
    files_json: &str,
    import_stmt: &sclc::Loc<sclc::ImportStmt>,
) -> ReplResult {
    let import_path: sclc::ModuleId = import_stmt
        .as_ref()
        .vars
        .iter()
        .map(|var| var.as_ref().name.clone())
        .collect();
    // Allow `Self` as an alias for the current package (`Playground`)
    let import_path = if import_path.as_slice().first().map(String::as_str) == Some("Self") {
        let mut segments = vec!["Playground".to_string()];
        segments.extend(import_path.as_slice()[1..].iter().cloned());
        sclc::ModuleId::new(segments)
    } else {
        import_path
    };
    let Some(alias) = import_stmt
        .as_ref()
        .vars
        .last()
        .map(|var| var.as_ref().name.clone())
    else {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: Some("Invalid import statement".to_string()),
        };
    };

    let file_map = parse_files_json(files_json);
    let repo = make_repo(files_json);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;
    for name in file_map.keys() {
        if name.ends_with(".scl") {
            let _ = package.open(name).await;
        }
    }

    // Resolve all transitive imports (not just the direct one)
    let mut diags = sclc::DiagList::new();
    match program.resolve_imports().await {
        Ok(diagnosed) => {
            diagnosed.unpack(&mut diags);
        }
        Err(e) => {
            return ReplResult {
                output: None,
                effects: Vec::new(),
                error: Some(format!("{e}")),
            };
        }
    }

    // Evaluate the imported module (program.evaluate wires up transitive imports)
    let value = match program.evaluate(&import_path, &state.eval).await {
        Ok(diagnosed_val) => diagnosed_val.into_inner(),
        Err(e) => {
            return ReplResult {
                output: None,
                effects: drain_effects(&mut state.effects_rx),
                error: Some(e.to_string()),
            };
        }
    };

    // Get the file_mod (already loaded by resolve_imports above)
    let resolve_result = program.resolve_import(&import_path).await;
    let Ok(diagnosed) = resolve_result else {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: Some(format!("{}", resolve_result.unwrap_err())),
        };
    };
    let file_mod_opt = diagnosed.unpack(&mut diags);
    let Some(file_mod) = file_mod_opt.cloned() else {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: Some(format!("Could not resolve import {import_path}")),
        };
    };

    // Type-check the imported module
    let checker = sclc::TypeChecker::new(&program);
    let type_env = sclc::TypeEnv::new().with_module_id(&import_path);
    let type_result = checker.check_file_mod(&type_env, &file_mod);
    let Ok(diagnosed_ty) = type_result else {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: Some(format!("{}", type_result.unwrap_err())),
        };
    };
    let ty = diagnosed_ty.unpack(&mut diags);

    if diags.iter().any(|d| d.level() == sclc::DiagLevel::Error) {
        return ReplResult {
            output: None,
            effects: Vec::new(),
            error: Some(collect_diagnostics(&diags).join("\n")),
        };
    }

    // Extract type-level exports
    let type_exports = checker
        .type_level_exports(&type_env, &file_mod)
        .into_inner();

    state.bindings.insert(alias.clone(), (ty, value));
    if type_exports.iter().next().is_some() {
        state
            .type_defs
            .insert(alias.clone(), sclc::Type::Record(type_exports));
    }

    let effects = drain_effects(&mut state.effects_rx);
    ReplResult {
        output: Some(format!("import {import_path}")),
        effects,
        error: None,
    }
}

fn repl_type_env<'a>(
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

fn repl_eval_env<'a>(
    bindings: &'a HashMap<String, (sclc::Type, sclc::TrackedValue)>,
    module_id: &'a sclc::ModuleId,
) -> sclc::EvalEnv<'a> {
    bindings.iter().fold(
        sclc::EvalEnv::new().with_module_id(module_id),
        |env, (name, (_, value))| env.with_local(name.as_str(), value.clone()),
    )
}
