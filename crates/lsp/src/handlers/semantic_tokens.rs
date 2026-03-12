use lsp_types::{
    SemanticToken, SemanticTokenType, SemanticTokens, SemanticTokensFullOptions,
    SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams, SemanticTokensResult,
    SemanticTokensServerCapabilities,
};
use sclc::{SourceRepo, TypeChecker};

use crate::convert::uri_to_path;
use crate::helpers::find_module_by_path;
use crate::{LanguageServer, OutgoingMessage, RequestId};

/// The token types we emit, in order. The index into this array is the token type ID.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,   // 0
    SemanticTokenType::VARIABLE,  // 1
    SemanticTokenType::FUNCTION,  // 2
    SemanticTokenType::PARAMETER, // 3
    SemanticTokenType::STRING,    // 4
    SemanticTokenType::NUMBER,    // 5
    SemanticTokenType::OPERATOR,  // 6
    SemanticTokenType::TYPE,      // 7
    SemanticTokenType::NAMESPACE, // 8
    SemanticTokenType::PROPERTY,  // 9
    SemanticTokenType::COMMENT,   // 10
];

const TT_KEYWORD: u32 = 0;
const TT_VARIABLE: u32 = 1;
const TT_FUNCTION: u32 = 2;
const TT_PARAMETER: u32 = 3;
const TT_STRING: u32 = 4;
const TT_NUMBER: u32 = 5;
const TT_TYPE: u32 = 7;
const TT_NAMESPACE: u32 = 8;
const TT_PROPERTY: u32 = 9;

pub fn semantic_tokens_capability() -> SemanticTokensServerCapabilities {
    SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
        legend: SemanticTokensLegend {
            token_types: TOKEN_TYPES.to_vec(),
            token_modifiers: vec![],
        },
        full: Some(SemanticTokensFullOptions::Bool(true)),
        range: None,
        work_done_progress_options: Default::default(),
    })
}

pub async fn handle_semantic_tokens_full<S: SourceRepo + 'static>(
    server: &LanguageServer<S>,
    id: RequestId,
    params: SemanticTokensParams,
) -> Vec<OutgoingMessage> {
    let uri = &params.text_document.uri;

    let Some(path) = uri_to_path(uri) else {
        return vec![OutgoingMessage::response(
            id,
            Option::<SemanticTokensResult>::None,
        )];
    };

    let Some(program) = server.last_program.as_ref() else {
        return vec![OutgoingMessage::response(
            id,
            Option::<SemanticTokensResult>::None,
        )];
    };

    let Some((_module_id, file_mod)) = find_module_by_path(program, &server.root_path, &path)
    else {
        return vec![OutgoingMessage::response(
            id,
            Option::<SemanticTokensResult>::None,
        )];
    };

    let mut collector = TokenCollector::new();

    // Collect known globals and imports to classify variables.
    let globals = file_mod.find_globals();
    let checker = TypeChecker::new(program);
    let imports = checker.find_imports(file_mod);

    let global_fns: std::collections::HashSet<&str> = globals
        .iter()
        .filter(|(_, expr)| matches!(expr.as_ref(), sclc::Expr::Fn(_)))
        .map(|(name, _)| *name)
        .collect();

    let import_names: std::collections::HashSet<&str> = imports.keys().copied().collect();

    for stmt in &file_mod.statements {
        collect_stmt_tokens(stmt, &global_fns, &import_names, &mut collector);
    }

    collector.sort();
    let tokens = collector.into_lsp_tokens();

    vec![OutgoingMessage::response(
        id,
        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })),
    )]
}

/// A raw token before delta-encoding.
struct RawToken {
    line: u32, // 1-based (sclc)
    col: u32,  // 1-based (sclc)
    length: u32,
    token_type: u32,
}

struct TokenCollector {
    tokens: Vec<RawToken>,
}

impl TokenCollector {
    fn new() -> Self {
        Self { tokens: Vec::new() }
    }

    fn push(&mut self, span: sclc::Span, token_type: u32) {
        let start = span.start();
        let end = span.end();
        // Only handle single-line tokens for simplicity.
        if start.line() == end.line() && end.character() > start.character() {
            self.tokens.push(RawToken {
                line: start.line(),
                col: start.character(),
                length: end.character() - start.character(),
                token_type,
            });
        }
    }

    fn sort(&mut self) {
        self.tokens
            .sort_by(|a, b| a.line.cmp(&b.line).then(a.col.cmp(&b.col)));
    }

    /// Convert to LSP delta-encoded tokens.
    fn into_lsp_tokens(self) -> Vec<SemanticToken> {
        let mut result = Vec::with_capacity(self.tokens.len());
        let mut prev_line: u32 = 0; // 0-based for LSP
        let mut prev_col: u32 = 0;

        for tok in &self.tokens {
            // Convert from 1-based to 0-based.
            let line_0 = tok.line.saturating_sub(1);
            let col_0 = tok.col.saturating_sub(1);

            let delta_line = line_0 - prev_line;
            let delta_start = if delta_line == 0 {
                col_0 - prev_col
            } else {
                col_0
            };

            result.push(SemanticToken {
                delta_line,
                delta_start,
                length: tok.length,
                token_type: tok.token_type,
                token_modifiers_bitset: 0,
            });

            prev_line = line_0;
            prev_col = col_0;
        }

        result
    }
}

fn collect_stmt_tokens(
    stmt: &sclc::ModStmt,
    global_fns: &std::collections::HashSet<&str>,
    import_names: &std::collections::HashSet<&str>,
    collector: &mut TokenCollector,
) {
    match stmt {
        sclc::ModStmt::Import(import) => {
            // The "import" keyword is not tracked in the AST span directly,
            // but the first var starts right after it. We emit the import vars as namespace tokens.
            for var in &import.vars {
                collector.push(var.span(), TT_NAMESPACE);
            }
        }
        sclc::ModStmt::Let(bind) => {
            // "let" keyword isn't in the AST with its own span; emit binding var.
            let ty = if global_fns.contains(bind.var.name.as_str()) {
                TT_FUNCTION
            } else {
                TT_VARIABLE
            };
            collector.push(bind.var.span(), ty);
            collect_expr_tokens(&bind.expr, global_fns, import_names, collector);
        }
        sclc::ModStmt::Export(bind) => {
            let ty = if global_fns.contains(bind.var.name.as_str()) {
                TT_FUNCTION
            } else {
                TT_VARIABLE
            };
            collector.push(bind.var.span(), ty);
            collect_expr_tokens(&bind.expr, global_fns, import_names, collector);
        }
        sclc::ModStmt::Expr(expr) => {
            collect_expr_tokens(expr, global_fns, import_names, collector);
        }
    }
}

fn collect_expr_tokens(
    expr: &sclc::Loc<sclc::Expr>,
    global_fns: &std::collections::HashSet<&str>,
    import_names: &std::collections::HashSet<&str>,
    collector: &mut TokenCollector,
) {
    use sclc::Expr;

    match expr.as_ref() {
        Expr::Int(_) | Expr::Float(_) => {
            collector.push(expr.span(), TT_NUMBER);
        }
        Expr::Bool(_) | Expr::Nil => {
            collector.push(expr.span(), TT_KEYWORD);
        }
        Expr::Str(_) => {
            collector.push(expr.span(), TT_STRING);
        }
        Expr::Var(var) => {
            let ty = if import_names.contains(var.name.as_str()) {
                TT_NAMESPACE
            } else if global_fns.contains(var.name.as_str()) {
                TT_FUNCTION
            } else {
                TT_VARIABLE
            };
            collector.push(var.span(), ty);
        }
        Expr::Fn(fn_expr) => {
            for param in &fn_expr.params {
                collector.push(param.var.span(), TT_PARAMETER);
                collect_type_expr_tokens(&param.ty, collector);
            }
            for tp in &fn_expr.type_params {
                collector.push(tp.var.span(), TT_TYPE);
                if let Some(bound) = &tp.bound {
                    collect_type_expr_tokens(bound, collector);
                }
            }
            collect_expr_tokens(&fn_expr.body, global_fns, import_names, collector);
        }
        Expr::Call(call) => {
            collect_expr_tokens(&call.callee, global_fns, import_names, collector);
            for ta in &call.type_args {
                collect_type_expr_tokens(ta, collector);
            }
            for arg in &call.args {
                collect_expr_tokens(arg, global_fns, import_names, collector);
            }
        }
        Expr::Let(let_expr) => {
            collector.push(let_expr.bind.var.span(), TT_VARIABLE);
            collect_expr_tokens(&let_expr.bind.expr, global_fns, import_names, collector);
            collect_expr_tokens(&let_expr.expr, global_fns, import_names, collector);
        }
        Expr::If(if_expr) => {
            collect_expr_tokens(&if_expr.condition, global_fns, import_names, collector);
            collect_expr_tokens(&if_expr.then_expr, global_fns, import_names, collector);
            if let Some(else_expr) = &if_expr.else_expr {
                collect_expr_tokens(else_expr, global_fns, import_names, collector);
            }
        }
        Expr::Binary(bin) => {
            collect_expr_tokens(&bin.lhs, global_fns, import_names, collector);
            collect_expr_tokens(&bin.rhs, global_fns, import_names, collector);
        }
        Expr::Unary(unary) => {
            collect_expr_tokens(&unary.expr, global_fns, import_names, collector);
        }
        Expr::Record(record) => {
            for field in &record.fields {
                collector.push(field.var.span(), TT_PROPERTY);
                collect_expr_tokens(&field.expr, global_fns, import_names, collector);
            }
        }
        Expr::Dict(dict) => {
            for entry in &dict.entries {
                collect_expr_tokens(&entry.key, global_fns, import_names, collector);
                collect_expr_tokens(&entry.value, global_fns, import_names, collector);
            }
        }
        Expr::List(list) => {
            for item in &list.items {
                collect_list_item_tokens(item, global_fns, import_names, collector);
            }
        }
        Expr::Interp(interp) => {
            for part in &interp.parts {
                collect_expr_tokens(part, global_fns, import_names, collector);
            }
        }
        Expr::PropertyAccess(prop) => {
            collect_expr_tokens(&prop.expr, global_fns, import_names, collector);
            collector.push(prop.property.span(), TT_PROPERTY);
        }
        Expr::Extern(ext) => {
            collect_type_expr_tokens(&ext.ty, collector);
        }
        Expr::Exception(_) => {}
        Expr::Raise(raise) => {
            collect_expr_tokens(&raise.expr, global_fns, import_names, collector);
        }
        Expr::Try(try_expr) => {
            collect_expr_tokens(&try_expr.expr, global_fns, import_names, collector);
            for catch in &try_expr.catches {
                collector.push(catch.exception_var.span(), TT_VARIABLE);
                if let Some(catch_arg) = &catch.catch_arg {
                    collector.push(catch_arg.span(), TT_PARAMETER);
                }
                collect_expr_tokens(&catch.body, global_fns, import_names, collector);
            }
        }
    }
}

fn collect_list_item_tokens(
    item: &sclc::ListItem,
    global_fns: &std::collections::HashSet<&str>,
    import_names: &std::collections::HashSet<&str>,
    collector: &mut TokenCollector,
) {
    match item {
        sclc::ListItem::Expr(expr) => {
            collect_expr_tokens(expr, global_fns, import_names, collector);
        }
        sclc::ListItem::If(if_item) => {
            collect_expr_tokens(&if_item.condition, global_fns, import_names, collector);
            collect_list_item_tokens(&if_item.then_item, global_fns, import_names, collector);
        }
        sclc::ListItem::For(for_item) => {
            collector.push(for_item.var.span(), TT_VARIABLE);
            collect_expr_tokens(&for_item.iterable, global_fns, import_names, collector);
            collect_list_item_tokens(&for_item.emit_item, global_fns, import_names, collector);
        }
    }
}

fn collect_type_expr_tokens(type_expr: &sclc::Loc<sclc::TypeExpr>, collector: &mut TokenCollector) {
    use sclc::TypeExpr;

    match type_expr.as_ref() {
        TypeExpr::Var(var) => {
            collector.push(var.span(), TT_TYPE);
        }
        TypeExpr::Optional(inner) => {
            collect_type_expr_tokens(inner, collector);
        }
        TypeExpr::List(inner) => {
            collect_type_expr_tokens(inner, collector);
        }
        TypeExpr::Fn(fn_ty) => {
            for tp in &fn_ty.type_params {
                collector.push(tp.var.span(), TT_TYPE);
                if let Some(bound) = &tp.bound {
                    collect_type_expr_tokens(bound, collector);
                }
            }
            for param in &fn_ty.params {
                collect_type_expr_tokens(param, collector);
            }
            collect_type_expr_tokens(&fn_ty.ret, collector);
        }
        TypeExpr::Record(record) => {
            for field in &record.fields {
                collector.push(field.var.span(), TT_PROPERTY);
                collect_type_expr_tokens(&field.ty, collector);
            }
        }
        TypeExpr::Dict(dict) => {
            collect_type_expr_tokens(&dict.key, collector);
            collect_type_expr_tokens(&dict.value, collector);
        }
    }
}
