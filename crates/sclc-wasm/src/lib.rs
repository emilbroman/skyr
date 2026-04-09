use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use wasm_bindgen::prelude::*;

fn make_repo(files_json: &str) -> sclc::MemSourceRepo {
    let file_map: HashMap<String, String> = serde_json::from_str(files_json).unwrap_or_default();
    sclc::MemSourceRepo::new(
        sclc::PackageId::from(["Playground"]),
        file_map
            .into_iter()
            .map(|(name, content)| (name, content.into_bytes()))
            .collect(),
    )
}

fn parse_files_json(files_json: &str) -> HashMap<String, String> {
    serde_json::from_str(files_json).unwrap_or_default()
}

/// Derive module ID from a file path relative to the package root.
/// e.g. "models/User.scl" -> ["Playground", "models", "User"]
fn module_id_for_file(file: &str) -> sclc::ModuleId {
    let path = Path::new(file);
    let mut path_segments: Vec<String> = Vec::new();
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            path_segments.push(component.as_os_str().to_string_lossy().into_owned());
        }
    }
    if let Some(stem) = path.file_stem() {
        path_segments.push(stem.to_string_lossy().into_owned());
    }
    sclc::ModuleId::new(sclc::PackageId::from(["Playground"]), path_segments)
}

/// Convert a module ID back to a file path relative to the package root.
/// e.g. ["Playground", "models", "User"] -> "models/User.scl"
fn file_for_module_id(module_id: &sclc::ModuleId) -> Option<String> {
    if module_id.path.is_empty() {
        return None;
    }
    let mut path = PathBuf::new();
    for s in &module_id.path {
        path.push(s);
    }
    path.set_extension("scl");
    Some(path.to_string_lossy().into_owned())
}

/// Load a compilation unit from multiple files (compile + type check), returning diagnostics.
async fn load_and_compile(files_json: &str) -> (sclc::DiagList, sclc::CompilationUnit) {
    let repo = make_repo(files_json);
    let mut diags = sclc::DiagList::new();

    match sclc::compile(repo).await {
        Ok(diagnosed) => {
            let unit = diagnosed.unpack(&mut diags);
            (diags, unit)
        }
        Err(error) => {
            eprintln!("sclc-wasm: compile failed: {error}");
            (diags, sclc::CompilationUnit::new())
        }
    }
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

    let package_id = sclc::PackageId::from(["Playground"]);

    let result: Vec<DiagnosticInfo> = diags
        .iter()
        .filter(|d| {
            let (module_id, _) = d.locate();
            module_id.package == package_id
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
    let (_, unit) = load_and_compile(files_json).await;

    let module_id = module_id_for_file(file);
    let source = file_map.get(file)?;
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&unit, source, &module_id, position);

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
    let (_, unit) = load_and_compile(files_json).await;

    let module_id = module_id_for_file(file);
    let Some(source) = file_map.get(file) else {
        return "[]".to_string();
    };
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&unit, source, &module_id, position);

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
            sclc::CompletionCandidate::PathFile(name) => CompletionItem {
                label: name.clone(),
                kind: "file",
                detail: None,
                description: None,
            },
            sclc::CompletionCandidate::PathDir(name) => CompletionItem {
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
    let (_, unit) = load_and_compile(files_json).await;

    let module_id = module_id_for_file(file);
    let source = file_map.get(file)?;
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&unit, source, &module_id, position);

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
    let module_id = sclc::ModuleId::new(
        sclc::PackageId::from(["Playground"]),
        vec!["Main".to_string()],
    );
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
    unit: &sclc::CompilationUnit,
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
    let checker = sclc::TypeChecker::new(unit);
    let _ = checker.check_file_mod(&type_env, &file_mod);

    cursor_info
}

// ---------------------------------------------------------------------------
// REPL support
// ---------------------------------------------------------------------------

struct WasmReplState {
    state: sclc::Repl,
    effects_rx: tokio::sync::mpsc::UnboundedReceiver<sclc::Effect>,
}

thread_local! {
    static REPL_STATE: RefCell<Option<WasmReplState>> = const { RefCell::new(None) };
}

/// Initialize a fresh REPL session.
#[wasm_bindgen]
pub fn repl_init() {
    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let program = sclc::Program::new();
    let state = sclc::Repl::new(program, effects_tx, "Playground".to_string());
    REPL_STATE.with(|cell| {
        *cell.borrow_mut() = Some(WasmReplState { state, effects_rx });
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

fn format_repl_error(err: &sclc::ReplError) -> String {
    match err {
        sclc::ReplError::Diagnostics(diags) => collect_diagnostics(diags).join("\n"),
        sclc::ReplError::TypeCheck(e) => e.to_string(),
        sclc::ReplError::Eval(e) => e.to_string(),
        sclc::ReplError::Resolve(e) => e.to_string(),
        sclc::ReplError::ResolveImport(e) => e.to_string(),
    }
}

/// Evaluate a REPL line. Returns JSON with { output?, effects?, error? }.
#[wasm_bindgen]
pub async fn repl_eval(files_json: &str, line: &str) -> String {
    // Take state out of thread-local
    let wasm_state = REPL_STATE.with(|cell| cell.borrow_mut().take());
    let Some(mut wasm_state) = wasm_state else {
        return r#"{"error":"REPL not initialized. Call repl_init() first."}"#.to_string();
    };

    let result = repl_process(&mut wasm_state, files_json, line).await;

    // Put state back
    REPL_STATE.with(|cell| {
        *cell.borrow_mut() = Some(wasm_state);
    });

    serde_json::to_string(&result)
        .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string())
}

async fn repl_process(wasm_state: &mut WasmReplState, files_json: &str, line: &str) -> ReplResult {
    wasm_state.state.replace_user_source(make_repo(files_json));
    let effects = drain_effects(&mut wasm_state.effects_rx);

    match wasm_state.state.process(line.to_string()).await {
        Ok(Some(sclc::ReplOutcome::Binding { name, ty })) => ReplResult {
            output: Some(format!("{name} : {ty}")),
            effects,
            error: None,
        },
        Ok(Some(sclc::ReplOutcome::Value { value })) => ReplResult {
            output: Some(format!("{}", value.value)),
            effects,
            error: None,
        },
        Ok(Some(sclc::ReplOutcome::TypeDef { name })) => ReplResult {
            output: Some(format!("type {name}")),
            effects: Vec::new(),
            error: None,
        },
        Ok(Some(sclc::ReplOutcome::Import { module_id })) => ReplResult {
            output: Some(format!("import {module_id}")),
            effects,
            error: None,
        },
        Ok(None) => ReplResult {
            output: None,
            effects,
            error: None,
        },
        Err(err) => ReplResult {
            output: None,
            effects,
            error: Some(format_repl_error(&err)),
        },
    }
}
