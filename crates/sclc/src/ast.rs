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
            Expr::Float(_) => HashSet::new(),
            Expr::Bool(_) => HashSet::new(),
            Expr::Nil => HashSet::new(),
            Expr::Str(_) => HashSet::new(),
            Expr::Extern(_) => HashSet::new(),
            Expr::If(if_expr) => {
                let mut vars = if_expr.condition.as_ref().free_vars();
                vars.extend(if_expr.then_expr.as_ref().free_vars());
                if let Some(else_expr) = &if_expr.else_expr {
                    vars.extend(else_expr.as_ref().free_vars());
                }
                vars
            }
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
            Expr::Unary(unary_expr) => unary_expr.expr.as_ref().free_vars(),
            Expr::Binary(binary_expr) => {
                let mut vars = binary_expr.lhs.as_ref().free_vars();
                vars.extend(binary_expr.rhs.as_ref().free_vars());
                vars
            }
            Expr::Record(record_expr) => {
                let mut vars = HashSet::new();
                for field in &record_expr.fields {
                    vars.extend(field.expr.as_ref().free_vars());
                }
                vars
            }
            Expr::Dict(dict_expr) => {
                let mut vars = HashSet::new();
                for entry in &dict_expr.entries {
                    vars.extend(entry.key.as_ref().free_vars());
                    vars.extend(entry.value.as_ref().free_vars());
                }
                vars
            }
            Expr::List(list_expr) => {
                let mut vars = HashSet::new();
                for item in &list_expr.items {
                    vars.extend(item.free_vars());
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
            Expr::Exception(_) => HashSet::new(),
            Expr::Raise(raise_expr) => raise_expr.expr.as_ref().free_vars(),
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
    Exception(ExceptionExpr),
    Raise(RaiseExpr),
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
pub struct ExternExpr {
    pub name: String,
    pub ty: Loc<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfExpr {
    pub condition: Box<Loc<Expr>>,
    pub then_expr: Box<Loc<Expr>>,
    pub else_expr: Option<Box<Loc<Expr>>>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Negate,
}

impl std::fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnaryOp::Negate => write!(f, "-"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnaryExpr {
    pub op: UnaryOp,
    pub expr: Box<Loc<Expr>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    And,
    Or,
}

impl std::fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryOp::Add => write!(f, "+"),
            BinaryOp::Sub => write!(f, "-"),
            BinaryOp::Mul => write!(f, "*"),
            BinaryOp::Div => write!(f, "/"),
            BinaryOp::Eq => write!(f, "=="),
            BinaryOp::Neq => write!(f, "!="),
            BinaryOp::Lt => write!(f, "<"),
            BinaryOp::Lte => write!(f, "<="),
            BinaryOp::Gt => write!(f, ">"),
            BinaryOp::Gte => write!(f, ">="),
            BinaryOp::And => write!(f, "&&"),
            BinaryOp::Or => write!(f, "||"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryExpr {
    pub op: BinaryOp,
    pub lhs: Box<Loc<Expr>>,
    pub rhs: Box<Loc<Expr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordExpr {
    pub fields: Vec<RecordField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictExpr {
    pub entries: Vec<DictEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListExpr {
    pub items: Vec<ListItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListItem {
    Expr(Loc<Expr>),
    If(ListIfItem),
    For(ListForItem),
}

impl ListItem {
    pub fn free_vars(&self) -> HashSet<&str> {
        match self {
            ListItem::Expr(expr) => expr.as_ref().free_vars(),
            ListItem::If(item) => {
                let mut vars = item.condition.as_ref().free_vars();
                vars.extend(item.then_item.as_ref().free_vars());
                vars
            }
            ListItem::For(item) => {
                let mut vars = item.iterable.as_ref().free_vars();
                let mut body_vars = item.emit_item.as_ref().free_vars();
                body_vars.remove(item.var.name.as_str());
                vars.extend(body_vars);
                vars
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListIfItem {
    pub condition: Box<Loc<Expr>>,
    pub then_item: Box<ListItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListForItem {
    pub var: Loc<Var>,
    pub iterable: Box<Loc<Expr>>,
    pub emit_item: Box<ListItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub var: Loc<Var>,
    pub expr: Loc<Expr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictEntry {
    pub key: Loc<Expr>,
    pub value: Loc<Expr>,
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
    Optional(Box<Loc<TypeExpr>>),
    List(Box<Loc<TypeExpr>>),
    Fn(FnTypeExpr),
    Record(RecordTypeExpr),
    Dict(DictTypeExpr),
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
pub struct DictTypeExpr {
    pub key: Box<Loc<TypeExpr>>,
    pub value: Box<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordTypeFieldExpr {
    pub var: Loc<Var>,
    pub ty: Loc<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExceptionExpr {
    pub exception_id: u64,
    pub ty: Option<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaiseExpr {
    pub expr: Box<Loc<Expr>>,
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
