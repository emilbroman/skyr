use crate::Loc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileMod {
    pub statements: Vec<ModStmt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModStmt {
    Import(Loc<ImportStmt>),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Var(Var),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Var {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportStmt {
    pub vars: Vec<Var>,
}
