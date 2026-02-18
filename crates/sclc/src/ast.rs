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
            match statement {
                ModStmt::Let(let_bind) | ModStmt::Export(let_bind) => {
                    globals.insert(let_bind.var.name.as_str(), let_bind.expr.as_ref());
                }
                _ => {}
            }
        }

        globals
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModStmt {
    Import(Loc<ImportStmt>),
    Let(LetBind),
    Export(LetBind),
    Print(PrintStmt),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Int(Int),
    Let(LetExpr),
    Var(Loc<Var>),
    Record(RecordExpr),
    PropertyAccess(PropertyAccessExpr),
}

#[derive(Clone, PartialEq, Eq)]
pub struct Var {
    pub name: String,
}

impl std::fmt::Debug for Var {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.name)
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyAccessExpr {
    pub expr: Box<Expr>,
    pub property: Loc<Var>,
}
