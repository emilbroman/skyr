use std::collections::{HashMap, HashSet};

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
    pub fn find_globals(&self) -> HashMap<&str, &Loc<Expr>> {
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

impl Expr {
    pub fn free_vars(&self) -> HashSet<&str> {
        match self {
            Expr::Int(_) => HashSet::new(),
            Expr::Bool(_) => HashSet::new(),
            Expr::Str(_) => HashSet::new(),
            Expr::Extern(_) => HashSet::new(),
            Expr::Var(var) => HashSet::from([var.name.as_str()]),
            Expr::Let(let_expr) => {
                let mut vars = let_expr.bind.expr.as_ref().free_vars();
                let mut body_vars = let_expr.expr.as_ref().free_vars();
                body_vars.remove(let_expr.bind.var.name.as_str());
                vars.extend(body_vars);
                vars
            }
            Expr::Fn(fn_expr) => {
                let mut vars = fn_expr.body.as_ref().free_vars();
                for param in &fn_expr.params {
                    vars.remove(param.var.name.as_str());
                }
                vars
            }
            Expr::Call(call_expr) => {
                let mut vars = call_expr.callee.as_ref().free_vars();
                for arg in &call_expr.args {
                    vars.extend(arg.as_ref().free_vars());
                }
                vars
            }
            Expr::Record(record_expr) => {
                let mut vars = HashSet::new();
                for field in &record_expr.fields {
                    vars.extend(field.expr.as_ref().free_vars());
                }
                vars
            }
            Expr::Interp(interp_expr) => {
                let mut vars = HashSet::new();
                for part in &interp_expr.parts {
                    vars.extend(part.as_ref().free_vars());
                }
                vars
            }
            Expr::PropertyAccess(property_access) => property_access.expr.as_ref().free_vars(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModStmt {
    Import(Loc<ImportStmt>),
    Let(LetBind),
    Export(LetBind),
    Expr(Loc<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Int(Int),
    Bool(Bool),
    Str(StrExpr),
    Extern(ExternExpr),
    Let(LetExpr),
    Fn(FnExpr),
    Call(CallExpr),
    Var(Loc<Var>),
    Record(RecordExpr),
    Interp(InterpExpr),
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
pub struct Bool {
    pub value: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrExpr {
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternExpr {
    pub name: String,
    pub ty: Loc<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportStmt {
    pub vars: Vec<Loc<Var>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetBind {
    pub var: Loc<Var>,
    pub expr: Box<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetExpr {
    pub bind: LetBind,
    pub expr: Box<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordExpr {
    pub fields: Vec<RecordField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub var: Loc<Var>,
    pub expr: Loc<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyAccessExpr {
    pub expr: Box<Loc<Expr>>,
    pub property: Loc<Var>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnExpr {
    pub params: Vec<FnParam>,
    pub body: Box<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnParam {
    pub var: Loc<Var>,
    pub ty: Loc<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeExpr {
    Var(Loc<Var>),
    Fn(FnTypeExpr),
    Record(RecordTypeExpr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnTypeExpr {
    pub params: Vec<Loc<TypeExpr>>,
    pub ret: Box<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordTypeExpr {
    pub fields: Vec<RecordTypeFieldExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordTypeFieldExpr {
    pub var: Loc<Var>,
    pub ty: Loc<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallExpr {
    pub callee: Box<Loc<Expr>>,
    pub args: Vec<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpExpr {
    pub parts: Vec<Loc<Expr>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> Loc<Var> {
        Loc::new(
            Var {
                name: name.to_owned(),
            },
            crate::Span::default(),
        )
    }

    #[test]
    fn free_vars_skip_fn_params() {
        let span = crate::Span::default();
        let expr_loc = |expr| Loc::new(expr, span);
        let type_loc = |ty| Loc::new(ty, span);
        let expr = Expr::Record(RecordExpr {
            fields: vec![
                RecordField {
                    var: var("a"),
                    expr: expr_loc(Expr::Var(var("a"))),
                },
                RecordField {
                    var: var("b"),
                    expr: expr_loc(Expr::Fn(FnExpr {
                        params: vec![
                            FnParam {
                                var: var("x"),
                                ty: type_loc(TypeExpr::Var(var("X"))),
                            },
                            FnParam {
                                var: var("y"),
                                ty: type_loc(TypeExpr::Var(var("Y"))),
                            },
                        ],
                        body: Box::new(expr_loc(Expr::Record(RecordExpr {
                            fields: vec![
                                RecordField {
                                    var: var("x"),
                                    expr: expr_loc(Expr::Var(var("x"))),
                                },
                                RecordField {
                                    var: var("y"),
                                    expr: expr_loc(Expr::Var(var("y"))),
                                },
                                RecordField {
                                    var: var("z"),
                                    expr: expr_loc(Expr::Var(var("z"))),
                                },
                            ],
                        }))),
                    })),
                },
            ],
        });

        let free_vars = expr.free_vars();
        assert_eq!(free_vars, HashSet::from(["a", "z"]));
    }
}
