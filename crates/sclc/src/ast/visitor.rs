use std::collections::HashSet;

use crate::Loc;

use super::{
    BinaryExpr, CallExpr, CatchClause, DictExpr, Expr, FileMod, FnExpr, IfExpr, IndexedAccessExpr,
    InterpExpr, LetBind, LetExpr, ListExpr, ListForItem, ListIfItem, ListItem, ModStmt, PathExpr,
    PropertyAccessExpr, RaiseExpr, RecordExpr, TryExpr, TypeCastExpr, UnaryExpr,
};

/// Trait for visiting AST nodes.
pub trait Visitor {
    fn visit_path(&mut self, path: &PathExpr);
}

/// Walk a `FileMod`, dispatching to the visitor for each `PathExpr`.
pub fn visit_file_mod(visitor: &mut dyn Visitor, file_mod: &FileMod) {
    for stmt in &file_mod.statements {
        visit_mod_stmt(visitor, stmt);
    }
}

fn visit_mod_stmt(visitor: &mut dyn Visitor, stmt: &ModStmt) {
    match stmt {
        ModStmt::Let(bind) | ModStmt::Export(bind) => visit_let_bind(visitor, bind),
        ModStmt::Expr(expr) => visit_expr(visitor, expr),
        ModStmt::Import(_) | ModStmt::TypeDef(_) | ModStmt::ExportTypeDef(_) => {}
    }
}

fn visit_let_bind(visitor: &mut dyn Visitor, bind: &LetBind) {
    visit_expr(visitor, &bind.expr);
}

fn visit_expr(visitor: &mut dyn Visitor, expr: &Loc<Expr>) {
    match expr.as_ref() {
        Expr::Path(path) => visitor.visit_path(path),
        Expr::Binary(e) => visit_binary(visitor, e),
        Expr::Call(e) => visit_call(visitor, e),
        Expr::Dict(e) => visit_dict(visitor, e),
        Expr::Fn(e) => visit_fn(visitor, e),
        Expr::If(e) => visit_if(visitor, e),
        Expr::IndexedAccess(e) => visit_indexed_access(visitor, e),
        Expr::Interp(e) => visit_interp(visitor, e),
        Expr::Let(e) => visit_let(visitor, e),
        Expr::List(e) => visit_list(visitor, e),
        Expr::PropertyAccess(e) => visit_property_access(visitor, e),
        Expr::Raise(e) => visit_raise(visitor, e),
        Expr::Record(e) => visit_record(visitor, e),
        Expr::Try(e) => visit_try(visitor, e),
        Expr::TypeCast(e) => visit_type_cast(visitor, e),
        Expr::Unary(e) => visit_unary(visitor, e),
        // Leaf nodes with no sub-expressions
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Bool(_)
        | Expr::Nil
        | Expr::Str(_)
        | Expr::Var(_)
        | Expr::Extern(_)
        | Expr::Exception(_) => {}
    }
}

fn visit_binary(visitor: &mut dyn Visitor, e: &BinaryExpr) {
    visit_expr(visitor, &e.lhs);
    visit_expr(visitor, &e.rhs);
}

fn visit_call(visitor: &mut dyn Visitor, e: &CallExpr) {
    visit_expr(visitor, &e.callee);
    for arg in &e.args {
        visit_expr(visitor, arg);
    }
}

fn visit_dict(visitor: &mut dyn Visitor, e: &DictExpr) {
    for entry in &e.entries {
        visit_expr(visitor, &entry.key);
        visit_expr(visitor, &entry.value);
    }
}

fn visit_fn(visitor: &mut dyn Visitor, e: &FnExpr) {
    if let Some(body) = &e.body {
        visit_expr(visitor, body);
    }
}

fn visit_if(visitor: &mut dyn Visitor, e: &IfExpr) {
    visit_expr(visitor, &e.condition);
    visit_expr(visitor, &e.then_expr);
    if let Some(else_expr) = &e.else_expr {
        visit_expr(visitor, else_expr);
    }
}

fn visit_indexed_access(visitor: &mut dyn Visitor, e: &IndexedAccessExpr) {
    visit_expr(visitor, &e.expr);
    visit_expr(visitor, &e.index);
}

fn visit_interp(visitor: &mut dyn Visitor, e: &InterpExpr) {
    for part in &e.parts {
        visit_expr(visitor, part);
    }
}

fn visit_let(visitor: &mut dyn Visitor, e: &LetExpr) {
    visit_let_bind(visitor, &e.bind);
    if let Some(body) = &e.expr {
        visit_expr(visitor, body);
    }
}

fn visit_list(visitor: &mut dyn Visitor, e: &ListExpr) {
    for item in &e.items {
        visit_list_item(visitor, item);
    }
}

fn visit_list_item(visitor: &mut dyn Visitor, item: &ListItem) {
    match item {
        ListItem::Expr(expr) => visit_expr(visitor, expr),
        ListItem::If(if_item) => visit_list_if(visitor, if_item),
        ListItem::For(for_item) => visit_list_for(visitor, for_item),
    }
}

fn visit_list_if(visitor: &mut dyn Visitor, item: &ListIfItem) {
    visit_expr(visitor, &item.condition);
    visit_list_item(visitor, &item.then_item);
}

fn visit_list_for(visitor: &mut dyn Visitor, item: &ListForItem) {
    visit_expr(visitor, &item.iterable);
    visit_list_item(visitor, &item.emit_item);
}

fn visit_property_access(visitor: &mut dyn Visitor, e: &PropertyAccessExpr) {
    visit_expr(visitor, &e.expr);
}

fn visit_raise(visitor: &mut dyn Visitor, e: &RaiseExpr) {
    visit_expr(visitor, &e.expr);
}

fn visit_record(visitor: &mut dyn Visitor, e: &RecordExpr) {
    for field in &e.fields {
        visit_expr(visitor, &field.expr);
    }
}

fn visit_try(visitor: &mut dyn Visitor, e: &TryExpr) {
    visit_expr(visitor, &e.expr);
    for catch in &e.catches {
        visit_catch(visitor, catch);
    }
}

fn visit_catch(visitor: &mut dyn Visitor, catch: &CatchClause) {
    visit_expr(visitor, &catch.body);
}

fn visit_type_cast(visitor: &mut dyn Visitor, e: &TypeCastExpr) {
    visit_expr(visitor, &e.expr);
}

fn visit_unary(visitor: &mut dyn Visitor, e: &UnaryExpr) {
    visit_expr(visitor, &e.expr);
}

/// Collects all unique `PathExpr` values from an AST.
#[derive(Default)]
pub struct CollectPaths {
    pub paths: HashSet<PathExpr>,
}

impl CollectPaths {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Visitor for CollectPaths {
    fn visit_path(&mut self, path: &PathExpr) {
        self.paths.insert(path.clone());
    }
}
