use std::collections::HashSet;

use ordered_float::NotNan;

use crate::eval::{Eval, EvalEnv, EvalError};
use crate::{
    DiagList, Diagnosed, Loc, SourceRepo, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
    TypeKind, Value,
};

use super::{
    BinaryExpr, CallExpr, DictExpr, ExceptionExpr, ExternExpr, FnExpr, IfExpr, IndexedAccessExpr,
    InterpExpr, LetExpr, ListExpr, PropertyAccessExpr, RaiseExpr, RecordExpr, TryExpr,
    TypeCastExpr, UnaryExpr, Var,
};

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

    /// Synthesis mode: bottom-up type inference with no expected type.
    pub(crate) fn type_synth<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match self {
            Expr::Int(_) => Ok(Diagnosed::new(Type::Int, DiagList::new())),
            Expr::Float(_) => Ok(Diagnosed::new(Type::Float, DiagList::new())),
            Expr::Bool(_) => Ok(Diagnosed::new(Type::Bool, DiagList::new())),
            Expr::Nil => Ok(Diagnosed::new(
                Type::Optional(Box::new(Type::Never)),
                DiagList::new(),
            )),
            Expr::Str(_) => Ok(Diagnosed::new(Type::Str, DiagList::new())),
            Expr::Extern(extern_expr) => extern_expr.type_synth(checker, env),
            Expr::If(if_expr) => if_expr.type_synth(checker, env, expr),
            Expr::Let(let_expr) => let_expr.type_synth(checker, env),
            Expr::Fn(fn_expr) => fn_expr.type_synth(checker, env),
            Expr::Call(call_expr) => call_expr.type_synth(checker, env, expr),
            Expr::Unary(unary_expr) => unary_expr.type_synth(checker, env, expr),
            Expr::Binary(binary_expr) => binary_expr.type_synth(checker, env, expr),
            Expr::Var(var) => super::var_expr::synth_var(checker, env, expr, var),
            Expr::Record(record_expr) => record_expr.type_synth(checker, env),
            Expr::Dict(dict_expr) => dict_expr.type_synth(checker, env),
            Expr::List(list_expr) => list_expr.type_synth(checker, env),
            Expr::Interp(interp_expr) => interp_expr.type_synth(checker, env),
            Expr::TypeCast(cast) => cast.type_synth(checker, env),
            Expr::PropertyAccess(pa) => pa.type_synth(checker, env),
            Expr::IndexedAccess(ia) => ia.type_synth(checker, env, expr),
            Expr::Exception(exc) => exc.type_synth(checker, env),
            Expr::Raise(raise) => raise.type_synth(checker, env),
            Expr::Try(try_expr) => try_expr.type_synth(checker, env),
        }
    }

    /// Check mode: validate expression against an expected type.
    pub(crate) fn type_check<S: SourceRepo>(
        &self,
        checker: &TypeChecker<'_, S>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected: &Type,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match self {
            Expr::Nil if matches!(expected.kind, TypeKind::Optional(_)) => {
                Ok(Diagnosed::new(expected.clone(), DiagList::new()))
            }
            Expr::Fn(fn_expr) => fn_expr.type_check(checker, env, expr, expected),
            Expr::Record(record_expr) => record_expr.type_check(checker, env, expr, expected),
            Expr::List(list_expr) => list_expr.type_check(checker, env, expr, expected),
            Expr::Dict(dict_expr) => dict_expr.type_check(checker, env, expr, expected),
            Expr::If(if_expr) => if_expr.type_check(checker, env, expr, expected),
            Expr::Let(let_expr) => let_expr.type_check(checker, env, expected),
            Expr::TypeCast(cast) => cast.type_check(checker, env, expr, expected),
            Expr::Call(call_expr) => call_expr.type_check(checker, env, expr, expected),
            Expr::Try(try_expr) => try_expr.type_check(checker, env, expr, expected),
            _ => checker.synth_then_subsume(env, expr, expected),
        }
    }

    /// Evaluate the expression.
    pub(crate) fn eval(
        &self,
        evaluator: &Eval,
        env: &EvalEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<TrackedValue, EvalError> {
        match self {
            Expr::Int(int) => Ok(Eval::tracked(Value::Int(int.value))),
            Expr::Float(float) => Ok(Eval::tracked(Value::Float(float.value))),
            Expr::Bool(bool) => Ok(Eval::tracked(Value::Bool(bool.value))),
            Expr::Nil => Ok(Eval::tracked(Value::Nil)),
            Expr::Str(str) => Ok(Eval::tracked(Value::Str(str.value.clone()))),
            Expr::Extern(extern_expr) => extern_expr.eval(evaluator, env, expr),
            Expr::If(if_expr) => if_expr.eval(evaluator, env, expr),
            Expr::Let(let_expr) => let_expr.eval(evaluator, env),
            Expr::Fn(fn_expr) => fn_expr.eval(evaluator, env, expr),
            Expr::Call(call_expr) => call_expr.eval(evaluator, env, expr),
            Expr::Unary(unary_expr) => unary_expr.eval(evaluator, env, expr),
            Expr::Binary(binary_expr) => binary_expr.eval(evaluator, env, expr),
            Expr::Var(var) => evaluator.eval_var_name(env, var.name.as_str()),
            Expr::Record(record_expr) => record_expr.eval(evaluator, env),
            Expr::Dict(dict_expr) => dict_expr.eval(evaluator, env),
            Expr::List(list_expr) => list_expr.eval(evaluator, env),
            Expr::Interp(interp_expr) => interp_expr.eval(evaluator, env),
            Expr::TypeCast(cast) => cast.eval(evaluator, env),
            Expr::PropertyAccess(pa) => pa.eval(evaluator, env),
            Expr::IndexedAccess(ia) => ia.eval(evaluator, env),
            Expr::Exception(exc) => exc.eval(evaluator, env, expr),
            Expr::Raise(raise) => raise.eval(evaluator, env, expr),
            Expr::Try(try_expr) => try_expr.eval(evaluator, env, expr),
        }
    }
}
