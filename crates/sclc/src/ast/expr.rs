use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use ordered_float::NotNan;

use crate::eval::{Eval, EvalEnv, EvalError};
use crate::{
    DiagList, Diagnosed, Loc, ModuleId, TrackedValue, Type, TypeCheckError, TypeChecker, TypeEnv,
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
    Path(PathExpr),
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

#[derive(Clone, Debug)]
pub struct PathSegment {
    pub value: String,
    pub cursor: Option<(crate::Cursor, usize)>,
}

impl PartialEq for PathSegment {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl Eq for PathSegment {}

impl Hash for PathSegment {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathExpr {
    pub segments: Vec<PathSegment>,
}

impl PathExpr {
    /// Iterate over segment values as `&str`.
    pub fn values(&self) -> impl Iterator<Item = &str> {
        self.segments.iter().map(|s| s.value.as_str())
    }

    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Resolve the path expression to an absolute path string relative to the
    /// repository root using the provided module context.
    ///
    /// Absolute paths (no `.`/`..` prefix) are rooted at the module's package.
    /// Relative paths are resolved against the directory containing the current
    /// module within its package.
    pub fn resolve_with_context(&self, module_id: &ModuleId) -> String {
        let is_relative = self.values().next().is_some_and(|s| s == "." || s == "..");

        let mut components: Vec<&str> = if is_relative {
            let dir = &module_id.path[..module_id.path.len().saturating_sub(1)];
            dir.iter().map(|s| s.as_str()).collect()
        } else {
            Vec::new()
        };

        for segment in self.values() {
            match segment {
                "." => {}
                ".." => {
                    components.pop();
                }
                s => components.push(s),
            }
        }

        format!("/{}", components.join("/"))
    }

    /// Resolve the path expression to an absolute path string relative to the
    /// repository root. Relative paths (starting with `.` or `..`) are resolved
    /// against the directory containing the current module. Absolute paths are
    /// returned as-is after normalisation.
    fn resolve(&self, _evaluator: &Eval<'_>, env: &EvalEnv<'_>) -> String {
        let module_id = env
            .module_id
            .expect("module_id should be set during evaluation");
        self.resolve_with_context(module_id)
    }
}

impl Expr {
    pub fn free_vars(&self) -> HashSet<&str> {
        match self {
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Bool(_)
            | Expr::Nil
            | Expr::Str(_)
            | Expr::Path(_) => HashSet::new(),
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
    pub(crate) fn type_synth(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        match self {
            Expr::Int(_) => Ok(crate::TypeSynth::new(Diagnosed::new(
                Type::Int(),
                DiagList::new(),
            ))),
            Expr::Float(_) => Ok(crate::TypeSynth::new(Diagnosed::new(
                Type::Float(),
                DiagList::new(),
            ))),
            Expr::Bool(_) => Ok(crate::TypeSynth::new(Diagnosed::new(
                Type::Bool(),
                DiagList::new(),
            ))),
            Expr::Nil => Ok(crate::TypeSynth::new(Diagnosed::new(
                Type::Optional(Box::new(Type::Never())),
                DiagList::new(),
            ))),
            Expr::Str(_) => Ok(crate::TypeSynth::new(Diagnosed::new(
                Type::Str(),
                DiagList::new(),
            ))),
            Expr::Path(path_expr) => {
                // Path validation is handled by the Loader; the TypeChecker
                // no longer carries a children cache for validation.
                checker.add_path_completions(env, path_expr);
                Ok(crate::TypeSynth::new(Diagnosed::new(
                    Type::Path(),
                    DiagList::new(),
                )))
            }
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
    pub(crate) fn type_check(
        &self,
        checker: &TypeChecker<'_>,
        env: &TypeEnv<'_>,
        expr: &Loc<Expr>,
        expected: &Type,
    ) -> Result<crate::TypeSynth, TypeCheckError> {
        match self {
            Expr::Nil if matches!(expected.kind, TypeKind::Optional(_)) => Ok(
                crate::TypeSynth::new(Diagnosed::new(expected.clone(), DiagList::new())),
            ),
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
        evaluator: &Eval<'_>,
        env: &EvalEnv<'_>,
        expr: &Loc<Expr>,
    ) -> Result<TrackedValue, EvalError> {
        match self {
            Expr::Int(int) => Ok(crate::eval::tracked(Value::Int(int.value))),
            Expr::Float(float) => Ok(crate::eval::tracked(Value::Float(float.value))),
            Expr::Bool(bool) => Ok(crate::eval::tracked(Value::Bool(bool.value))),
            Expr::Nil => Ok(crate::eval::tracked(Value::Nil)),
            Expr::Str(str) => Ok(crate::eval::tracked(Value::Str(str.value.clone()))),
            Expr::Path(path) => {
                let resolved = path.resolve(evaluator, env);
                let package_id = env
                    .module_id
                    .map(|m| &m.package)
                    .cloned()
                    .unwrap_or_default();
                let hash = evaluator.resolve_path_hash(&resolved, &package_id);
                Ok(crate::eval::tracked(Value::Path(crate::PathValue {
                    path: resolved,
                    hash,
                })))
            }
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
            Expr::PropertyAccess(pa) => pa.eval(evaluator, env, expr),
            Expr::IndexedAccess(ia) => ia.eval(evaluator, env, expr),
            Expr::Exception(exc) => exc.eval(evaluator, env, expr),
            Expr::Raise(raise) => raise.eval(evaluator, env, expr),
            Expr::Try(try_expr) => try_expr.eval(evaluator, env, expr),
        }
    }
}
