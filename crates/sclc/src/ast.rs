use std::collections::HashMap;

use crate::Loc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileMod {
    pub statements: Vec<ModStmt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplLine {
    pub statement: ModStmt,
}

impl FileMod {
    pub fn find_globals(&self) -> HashMap<&str, &Expr> {
        let mut globals = HashMap::new();

        for statement in &self.statements {
            if let ModStmt::Let(let_bind) = statement {
                globals.insert(let_bind.var.name.as_str(), let_bind.expr.as_ref());
            }
        }

        globals
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModStmt {
    Import(Loc<ImportStmt>),
    Let(LetBind),
    Print(PrintStmt),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Int(Int),
    Let(LetExpr),
    Var(Loc<Var>),
    Record(RecordExpr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Var {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Int {
    pub value: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportStmt {
    pub vars: Vec<Loc<Var>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintStmt {
    pub expr: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetBind {
    pub var: Loc<Var>,
    pub expr: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetExpr {
    pub bind: LetBind,
    pub expr: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordExpr {
    pub fields: Vec<RecordField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub var: Loc<Var>,
    pub expr: Expr,
}
