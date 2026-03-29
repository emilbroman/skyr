use std::convert::Infallible;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use wasm_bindgen::prelude::*;

/// In-memory source repository holding a single `Main.scl` file.
struct MemSourceRepo {
    source: Vec<u8>,
}

impl sclc::SourceRepo for MemSourceRepo {
    type Err = Infallible;

    fn package_id(&self) -> sclc::ModuleId {
        ["Playground"].into_iter().map(String::from).collect()
    }

    async fn read_file(&self, path: &Path) -> Result<Option<Vec<u8>>, Self::Err> {
        if path == Path::new("Main.scl") {
            Ok(Some(self.source.clone()))
        } else {
            Ok(None)
        }
    }
}

fn make_source(source: &str) -> MemSourceRepo {
    MemSourceRepo {
        source: source.as_bytes().to_vec(),
    }
}

/// Load a program from source (compile + type check), returning diagnostics.
async fn load_and_compile(source: &str) -> (sclc::DiagList, sclc::Program<MemSourceRepo>) {
    let repo = make_source(source);
    let mut diags = sclc::DiagList::new();

    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;
    let _ = package.open("Main.scl").await;
    let _ = program.resolve_imports().await;

    if let Ok(diagnosed) = program.check_types() {
        diagnosed.unpack(&mut diags);
    }

    (diags, program)
}

#[derive(Serialize)]
struct DiagnosticInfo {
    line: u32,
    character: u32,
    end_line: u32,
    end_character: u32,
    message: String,
    severity: &'static str,
}

/// Analyze source code and return diagnostics as JSON.
#[wasm_bindgen]
pub async fn analyze(source: &str) -> String {
    let (diags, _) = load_and_compile(source).await;

    let package_id: sclc::ModuleId = ["Playground"].into_iter().map(String::from).collect();

    let result: Vec<DiagnosticInfo> = diags
        .iter()
        .filter(|d| {
            let (module_id, _) = d.locate();
            module_id.starts_with(&package_id)
        })
        .map(|d| {
            let (_, span) = d.locate();
            let level = d.level();
            DiagnosticInfo {
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

/// Get hover information (type + description) at a position.
#[wasm_bindgen]
pub async fn hover(source: &str, line: u32, col: u32) -> Option<String> {
    let repo = make_source(source);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;
    let _ = package.open("Main.scl").await;
    let _ = program.resolve_imports().await;

    let module_id: sclc::ModuleId = ["Playground", "Main"]
        .into_iter()
        .map(String::from)
        .collect();
    // LSP uses 0-based, sclc uses 1-based
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
}

/// Get completions at a position.
#[wasm_bindgen]
pub async fn completions(source: &str, line: u32, col: u32) -> String {
    let repo = make_source(source);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;
    let _ = package.open("Main.scl").await;
    let _ = program.resolve_imports().await;

    let module_id: sclc::ModuleId = ["Playground", "Main"]
        .into_iter()
        .map(String::from)
        .collect();
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
            },
            sclc::CompletionCandidate::Member(name) => CompletionItem {
                label: name.clone(),
                kind: "field",
            },
        })
        .collect();

    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Serialize)]
struct LocationInfo {
    line: u32,
    character: u32,
    end_line: u32,
    end_character: u32,
}

/// Get go-to-definition location at a position.
#[wasm_bindgen]
pub async fn goto_definition(source: &str, line: u32, col: u32) -> Option<String> {
    let repo = make_source(source);
    let mut program = sclc::Program::new();
    let package = program.open_package(repo).await;
    let _ = package.open("Main.scl").await;
    let _ = program.resolve_imports().await;

    let module_id: sclc::ModuleId = ["Playground", "Main"]
        .into_iter()
        .map(String::from)
        .collect();
    let position = sclc::Position::new(line + 1, col + 1);
    let cursor_info = query_cursor(&program, source, &module_id, position);

    let info = cursor_info.lock().unwrap();
    info.declaration.map(|span| {
        serde_json::to_string(&LocationInfo {
            line: span.start().line().saturating_sub(1),
            character: span.start().character().saturating_sub(1),
            end_line: span.end().line().saturating_sub(1),
            end_character: span.end().character().saturating_sub(1),
        })
        .unwrap()
    })
}

/// Format source code.
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
