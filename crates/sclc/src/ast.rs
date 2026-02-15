use crate::Loc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileMod {
    pub statements: Vec<ModStmt>,
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
    Var(Var),
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
    pub vars: Vec<Var>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrintStmt {
    pub expr: Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetBind {
    pub var: Var,
    pub expr: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetExpr {
    pub bind: LetBind,
    pub expr: Box<Expr>,
}
