mod binary_expr;
mod call_expr;
mod dict_expr;
mod exception_expr;
mod extern_expr;
mod fn_expr;
mod if_expr;
mod indexed_access;
mod interp_expr;
mod let_expr;
mod list_expr;
mod property_access;
mod raise_expr;
mod record_expr;
mod try_expr;
mod type_cast;
mod type_expr;
mod unary_expr;
pub(crate) mod var_expr;

pub use binary_expr::*;
pub use call_expr::*;
pub use dict_expr::*;
pub use exception_expr::*;
pub use extern_expr::*;
pub use fn_expr::*;
pub use if_expr::*;
pub use indexed_access::*;
pub use interp_expr::*;
pub use let_expr::*;
pub use list_expr::*;
pub use property_access::*;
pub use raise_expr::*;
pub use record_expr::*;
pub use try_expr::*;
pub use type_cast::*;
pub use type_expr::*;
pub use unary_expr::*;

use std::collections::{HashMap, HashSet};

use ordered_float::NotNan;

use crate::Loc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileMod {
    pub statements: Vec<ModStmt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplLine {
    pub statement: Option<ModStmt>,
}

impl FileMod {
    pub fn find_globals(&self) -> HashMap<&str, (crate::Span, &Loc<Expr>)> {
        let mut globals = HashMap::new();

        for statement in &self.statements {
            match statement {
                ModStmt::Let(let_bind) | ModStmt::Export(let_bind) => {
                    globals.insert(
                        let_bind.var.name.as_str(),
                        (let_bind.var.span(), let_bind.expr.as_ref()),
                    );
                }
                _ => {}
            }
        }

        globals
    }

    pub fn find_type_defs(&self) -> Vec<&TypeDef> {
        self.statements
            .iter()
            .filter_map(|stmt| match stmt {
                ModStmt::TypeDef(type_def) | ModStmt::ExportTypeDef(type_def) => Some(type_def),
                _ => None,
            })
            .collect()
    }
}

impl Expr {
    pub fn free_vars(&self) -> HashSet<&str> {
        match self {
            Expr::Int(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Nil | Expr::Str(_) => {
                HashSet::new()
            }
            Expr::Extern(_) | Expr::Exception(_) => HashSet::new(),
            Expr::If(e) => e.free_vars(),
            Expr::Let(e) => e.free_vars(),
            Expr::Fn(e) => e.free_vars(),
            Expr::Call(e) => e.free_vars(),
            Expr::Unary(e) => e.free_vars(),
            Expr::Binary(e) => e.free_vars(),
            Expr::Var(var) => HashSet::from([var.name.as_str()]),
            Expr::Record(e) => e.free_vars(),
            Expr::Dict(e) => e.free_vars(),
            Expr::List(e) => e.free_vars(),
            Expr::Interp(e) => e.free_vars(),
            Expr::PropertyAccess(e) => e.free_vars(),
            Expr::IndexedAccess(e) => e.free_vars(),
            Expr::TypeCast(e) => e.free_vars(),
            Expr::Raise(e) => e.free_vars(),
            Expr::Try(e) => e.free_vars(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModStmt {
    Import(Loc<ImportStmt>),
    Let(LetBind),
    Export(LetBind),
    TypeDef(TypeDef),
    ExportTypeDef(TypeDef),
    Expr(Loc<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Int(Int),
    Float(Float),
    Bool(Bool),
    Nil,
    Str(StrExpr),
    Extern(ExternExpr),
    If(IfExpr),
    Let(LetExpr),
    Fn(FnExpr),
    Call(CallExpr),
    Unary(UnaryExpr),
    Binary(BinaryExpr),
    Var(Loc<Var>),
    Record(RecordExpr),
    Dict(DictExpr),
    List(ListExpr),
    Interp(InterpExpr),
    PropertyAccess(PropertyAccessExpr),
    IndexedAccess(IndexedAccessExpr),
    TypeCast(TypeCastExpr),
    Exception(ExceptionExpr),
    Raise(RaiseExpr),
    Try(TryExpr),
}

#[derive(Clone)]
pub struct Var {
    pub name: String,
    pub cursor: Option<(crate::Cursor, usize)>,
}

impl PartialEq for Var {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Var {}

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
pub struct Float {
    pub value: NotNan<f64>,
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
pub struct ImportStmt {
    pub vars: Vec<Loc<Var>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetBind {
    pub var: Loc<Var>,
    pub ty: Option<Loc<TypeExpr>>,
    pub expr: Box<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeExpr {
    Var(Loc<Var>),
    Optional(Box<Loc<TypeExpr>>),
    List(Box<Loc<TypeExpr>>),
    Fn(FnTypeExpr),
    Record(RecordTypeExpr),
    Dict(DictTypeExpr),
    PropertyAccess(TypePropertyAccessExpr),
    Application(TypeApplicationExpr),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str) -> Loc<Var> {
        Loc::new(
            Var {
                name: name.to_owned(),
                cursor: None,
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
                        type_params: vec![],
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
