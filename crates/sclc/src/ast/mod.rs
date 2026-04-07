mod binary_expr;
mod call_expr;
mod dict_expr;
mod exception_expr;
mod expr;
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
mod visitor;

pub use binary_expr::*;
pub use call_expr::*;
pub use dict_expr::*;
pub use exception_expr::*;
pub use expr::*;
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
pub use visitor::*;

use std::collections::HashMap;

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
    pub fn find_globals(&self) -> HashMap<&str, (crate::Span, &Loc<Expr>, Option<&str>)> {
        let mut globals = HashMap::new();

        for statement in &self.statements {
            match statement {
                ModStmt::Let(let_bind) | ModStmt::Export(let_bind) => {
                    globals.insert(
                        let_bind.var.name.as_str(),
                        (
                            let_bind.var.span(),
                            let_bind.expr.as_ref(),
                            let_bind.doc_comment.as_deref(),
                        ),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModStmt {
    Import(Loc<ImportStmt>),
    Let(LetBind),
    Export(LetBind),
    TypeDef(TypeDef),
    ExportTypeDef(TypeDef),
    Expr(Loc<Expr>),
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
pub struct ImportStmt {
    pub vars: Vec<Loc<Var>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetBind {
    pub doc_comment: Option<String>,
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
    use std::collections::HashSet;

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
                    doc_comment: None,
                    var: var("a"),
                    expr: expr_loc(Expr::Var(var("a"))),
                },
                RecordField {
                    doc_comment: None,
                    var: var("b"),
                    expr: expr_loc(Expr::Fn(FnExpr {
                        type_params: vec![],
                        params: vec![
                            FnParam {
                                var: var("x"),
                                ty: Some(type_loc(TypeExpr::Var(var("X")))),
                            },
                            FnParam {
                                var: var("y"),
                                ty: Some(type_loc(TypeExpr::Var(var("Y")))),
                            },
                        ],
                        body: Some(Box::new(expr_loc(Expr::Record(RecordExpr {
                            fields: vec![
                                RecordField {
                                    doc_comment: None,
                                    var: var("x"),
                                    expr: expr_loc(Expr::Var(var("x"))),
                                },
                                RecordField {
                                    doc_comment: None,
                                    var: var("y"),
                                    expr: expr_loc(Expr::Var(var("y"))),
                                },
                                RecordField {
                                    doc_comment: None,
                                    var: var("z"),
                                    expr: expr_loc(Expr::Var(var("z"))),
                                },
                            ],
                        })))),
                    })),
                },
            ],
        });

        let free_vars = expr.free_vars();
        assert_eq!(free_vars, HashSet::from(["a", "z"]));
    }
}
