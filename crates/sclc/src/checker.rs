use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Component;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    AnySource, DiagList, Diagnosed, DictType, FnType, Package, Program, RecordType, Type, TypeKind,
    ast,
};
use thiserror::Error;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variance {
    Covariant,
    Contravariant,
}

impl Variance {
    fn flip(self) -> Self {
        match self {
            Variance::Covariant => Variance::Contravariant,
            Variance::Contravariant => Variance::Covariant,
        }
    }
}

/// Accumulates lower-bound constraints for free type variables during recursive
/// global checking. Shared (via `Rc<RefCell<…>>`) across all derived environments
/// while checking a single global's body.
#[derive(Default)]
struct FreeVarConstraints {
    /// Maps free var ID → accumulated lower bound.
    /// Starts as `Type::Never` and is tightened upward as constraints arrive.
    lower_bounds: HashMap<usize, Type>,
}

impl FreeVarConstraints {
    fn new() -> Self {
        Self::default()
    }

    /// Register a new free variable with initial lower bound `Never`.
    fn register(&mut self, id: usize) {
        self.lower_bounds.insert(id, Type::Never);
    }

    /// Returns true if `id` is a tracked free variable.
    fn contains(&self, id: usize) -> bool {
        self.lower_bounds.contains_key(&id)
    }

    /// Tighten the lower bound for a free variable by replacing `Never` with
    /// the first concrete constraint. Subsequent constraints are merged
    /// structurally when possible (e.g. record types are merged field-by-field).
    fn constrain(&mut self, id: usize, new_lower: Type) {
        let entry = self
            .lower_bounds
            .get_mut(&id)
            .expect("free var must be registered");
        if matches!(entry.kind, TypeKind::Never) {
            *entry = new_lower;
        } else if !matches!(new_lower.kind, TypeKind::Never) {
            // Attempt a structural merge for records: union the fields.
            match (&mut entry.kind, &new_lower.kind) {
                (TypeKind::Record(existing), TypeKind::Record(new_rec)) => {
                    for (name, ty) in new_rec.iter() {
                        if existing.get(name).is_none() {
                            existing.insert(name.clone(), ty.clone());
                        }
                    }
                }
                _ => {
                    // For non-record types, keep the first constraint.
                    // A more sophisticated system would compute a join.
                }
            }
        }
    }

    /// Solve constraints given that `primary_id` equals `body_type`.
    ///
    /// For each secondary free variable that appears in a constraint, try to
    /// find its concrete type by matching the constraint against the body type.
    /// Then produce substitution pairs. For the primary variable, if its lower
    /// bound references itself, wrap in IsoRec.
    fn solve(&self, primary_id: usize, body_type: &Type) -> Vec<(usize, Type)> {
        let mut solutions: HashMap<usize, Type> = HashMap::new();

        // The primary free variable equals the body type.
        // Walk the constraints for the primary and unify secondary vars.
        if let Some(constraint) = self.lower_bounds.get(&primary_id) {
            Self::unify_constraint(constraint, body_type, &mut solutions);
        }

        // Build the substitution list
        self.lower_bounds
            .iter()
            .map(|(id, bound)| {
                if *id == primary_id {
                    // Primary: use Never (it will be wrapped in IsoRec by the caller)
                    (*id, Type::Never)
                } else if let Some(solved) = solutions.get(id) {
                    (*id, solved.clone())
                } else if matches!(bound.kind, TypeKind::Never) {
                    (*id, Type::Never)
                } else {
                    (*id, bound.clone())
                }
            })
            .collect()
    }

    /// Walk a constraint type and a concrete type in parallel, recording
    /// solutions for free variables found in the constraint.
    fn unify_constraint(constraint: &Type, concrete: &Type, solutions: &mut HashMap<usize, Type>) {
        match (&constraint.kind, &concrete.kind) {
            (TypeKind::Var(id), _) => {
                solutions.entry(*id).or_insert_with(|| concrete.clone());
            }
            (TypeKind::Record(c_rec), TypeKind::Record(b_rec)) => {
                for (name, c_field) in c_rec.iter() {
                    if let Some(b_field) = b_rec.get(name) {
                        Self::unify_constraint(c_field, b_field, solutions);
                    }
                }
            }
            (TypeKind::Fn(c_fn), TypeKind::Fn(b_fn)) if c_fn.params.len() == b_fn.params.len() => {
                for (cp, bp) in c_fn.params.iter().zip(b_fn.params.iter()) {
                    Self::unify_constraint(cp, bp, solutions);
                }
                Self::unify_constraint(c_fn.ret.as_ref(), b_fn.ret.as_ref(), solutions);
            }
            (TypeKind::List(c_inner), TypeKind::List(b_inner)) => {
                Self::unify_constraint(c_inner, b_inner, solutions);
            }
            (TypeKind::Optional(c_inner), TypeKind::Optional(b_inner)) => {
                Self::unify_constraint(c_inner, b_inner, solutions);
            }
            (TypeKind::Dict(c_dict), TypeKind::Dict(b_dict)) => {
                Self::unify_constraint(c_dict.key.as_ref(), b_dict.key.as_ref(), solutions);
                Self::unify_constraint(c_dict.value.as_ref(), b_dict.value.as_ref(), solutions);
            }
            _ => {}
        }
    }
}

/// Heap-allocated maps that make up the mutable portion of a type environment.
/// Boxed to keep `TypeEnv` small on the stack (~48 bytes instead of ~224).
#[derive(Clone, Debug)]
struct TypeEnvMaps<'a> {
    locals: HashMap<&'a str, (crate::Span, Type)>,
    type_vars: HashMap<String, Type>,
    /// Upper bounds for type variable IDs (used during function body checking).
    type_var_bounds: HashMap<usize, Type>,
    /// Type-level bindings from `type` declarations and imports (separate namespace from values).
    type_level: HashMap<String, Type>,
}

pub struct TypeEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, (crate::Span, &'a crate::Loc<ast::Expr>)>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>>,
    maps: Box<TypeEnvMaps<'a>>,
    /// Cursor for reference tracking. Shared (via Arc) across all derived envs.
    cursor: Option<crate::Cursor>,
    /// Free variable constraints for recursive global checking.
    /// When set, free variables encountered during checking accumulate constraints here.
    free_vars: Option<Rc<RefCell<FreeVarConstraints>>>,
}

impl<'a> Default for TypeEnv<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> TypeEnv<'a> {
    pub fn new() -> Self {
        Self {
            module_id: None,
            globals: None,
            imports: None,
            maps: Box::new(TypeEnvMaps {
                locals: HashMap::new(),
                type_vars: HashMap::new(),
                type_var_bounds: HashMap::new(),
                type_level: HashMap::new(),
            }),
            cursor: None,
            free_vars: None,
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            maps: self.maps.clone(),
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn with_globals(
        &self,
        globals: &'a HashMap<&'a str, (crate::Span, &'a crate::Loc<ast::Expr>)>,
    ) -> Self {
        let mut maps = self.maps.clone();
        maps.locals = HashMap::new();
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            imports: self.imports,
            maps,
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn with_imports(
        &self,
        imports: &'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>,
    ) -> Self {
        let mut maps = self.maps.clone();
        maps.locals = HashMap::new();
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: Some(imports),
            maps,
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            imports: self.imports,
            maps: self.maps.clone(),
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn with_cursor(&self, cursor: crate::Cursor) -> Self {
        let mut env = self.inner();
        env.cursor = Some(cursor);
        env
    }

    pub fn with_local(&self, name: &'a str, span: crate::Span, ty: Type) -> Self {
        let mut env = self.inner();
        env.maps.locals.insert(name, (span, ty));
        env
    }

    pub fn with_type_var(&self, name: String, ty: Type) -> Self {
        let mut env = self.inner();
        env.maps.type_vars.insert(name, ty);
        env
    }

    pub fn with_type_var_bound(&self, id: usize, upper_bound: Type) -> Self {
        let mut env = self.inner();
        env.maps.type_var_bounds.insert(id, upper_bound);
        env
    }

    pub fn with_type_level(&self, name: String, ty: Type) -> Self {
        let mut env = self.inner();
        env.maps.type_level.insert(name, ty);
        env
    }

    /// Create a derived environment with a free variable for recursive global
    /// checking. The free variable is added to the shared constraint set and
    /// bound as a local so that name resolution finds it.
    fn with_free_var(
        &self,
        name: &'a str,
        span: crate::Span,
        type_id: usize,
        constraints: Rc<RefCell<FreeVarConstraints>>,
    ) -> Self {
        constraints.borrow_mut().register(type_id);
        let mut env = self.inner();
        env.maps.locals.insert(name, (span, Type::Var(type_id)));
        env.free_vars = Some(constraints);
        env
    }

    /// If `ty` is a type variable with a known upper bound, return a reference
    /// to the bound. Otherwise, return the passed-in reference unchanged.
    pub fn resolve_var_bound<'t>(&'t self, ty: &'t Type) -> &'t Type {
        if let TypeKind::Var(id) = ty.kind
            && let Some(bound) = self.maps.type_var_bounds.get(&id)
        {
            return bound;
        }
        ty
    }

    /// Check if a type variable ID is a free variable in the current constraint set.
    fn is_free_var(&self, id: usize) -> bool {
        self.free_vars
            .as_ref()
            .is_some_and(|fv| fv.borrow().contains(id))
    }

    pub fn without_locals(&self) -> Self {
        let mut maps = self.maps.clone();
        maps.locals = HashMap::new();
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            maps,
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn lookup_type_var(&self, name: &str) -> Option<&Type> {
        self.maps.type_vars.get(name)
    }

    pub fn lookup_type_level(&self, name: &str) -> Option<&Type> {
        self.maps.type_level.get(name)
    }

    pub fn lookup_local(&self, name: &str) -> Option<&(crate::Span, Type)> {
        self.maps.locals.get(name)
    }

    pub fn lookup_global(&self, name: &str) -> Option<(crate::Span, &crate::Loc<ast::Expr>)> {
        self.globals
            .and_then(|globals| globals.get(name))
            .map(|(span, expr)| (*span, *expr))
    }

    pub fn lookup_import(&self, name: &str) -> Option<(crate::ModuleId, Option<&'a ast::FileMod>)> {
        self.imports
            .and_then(|imports| imports.get(name))
            .map(|(module_id, file_mod)| (module_id.clone(), *file_mod))
    }

    pub fn module_id(&self) -> Result<crate::ModuleId, TypeCheckError> {
        self.module_id
            .cloned()
            .ok_or(TypeCheckError::ModuleIdMissing)
    }

    pub fn local_names(&self) -> impl Iterator<Item = &str> {
        self.maps.locals.keys().copied()
    }

    pub fn global_names(&self) -> impl Iterator<Item = &str> {
        self.globals
            .into_iter()
            .flat_map(|globals| globals.keys().copied())
    }
}

pub struct TypeChecker<'p, S> {
    program: &'p Program<S>,
    /// Cache for resolved global expression types (keyed by expression pointer).
    /// Avoids re-checking the same global expression multiple times within a
    /// single type-checking pass. Diagnostics are not cached — they are only
    /// emitted during the canonical check in `check_global_let_bind`.
    global_cache: RefCell<HashMap<*const crate::Loc<ast::Expr>, Type>>,
    /// Cache for resolved import module types (keyed by FileMod pointer).
    import_cache: RefCell<HashMap<*const ast::FileMod, Type>>,
    /// Cache for type-level exports (keyed by FileMod pointer).
    type_level_cache: RefCell<HashMap<*const ast::FileMod, RecordType>>,
}

/// Global monotonic counter for unique type variable IDs.
///
/// # Thread safety
///
/// The counter is process-global and atomic, so IDs are unique across threads.
/// However, the counter is never reset between compilations, meaning type IDs
/// will grow monotonically across invocations within the same process. This is
/// harmless in practice — IDs are only used for identity comparison during a
/// single type-checking pass — but embedders should be aware that IDs are not
/// stable or reproducible across runs.
static NEXT_TYPE_ID: AtomicUsize = AtomicUsize::new(0);

fn next_type_id() -> usize {
    NEXT_TYPE_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Error, Debug)]
#[error("undefined variable: {name}")]
pub struct UndefinedVariable {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub var: crate::Loc<ast::Var>,
}

impl crate::Diag for UndefinedVariable {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.var.span())
    }
}

#[derive(Error, Debug)]
#[error("undefined member: {name} in type {ty}")]
pub struct UndefinedMember {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub ty: Type,
    pub property: crate::Loc<ast::Var>,
}

impl crate::Diag for UndefinedMember {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.property.span())
    }
}

#[derive(Error, Debug)]
#[error("indexed access requires a Dict or List type, got {ty}")]
pub struct InvalidIndexTarget {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for InvalidIndexTarget {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("not a function: {ty}")]
pub struct NotAFunction {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for NotAFunction {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("missing arguments: expected {expected}, got {got}")]
pub struct MissingArguments {
    pub module_id: crate::ModuleId,
    pub expected: usize,
    pub got: usize,
    pub span: crate::Span,
}

impl crate::Diag for MissingArguments {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("extraneous argument at index {index}")]
pub struct ExtraneousArgument {
    pub module_id: crate::ModuleId,
    pub index: usize,
    pub span: crate::Span,
}

impl crate::Diag for ExtraneousArgument {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("type mismatch: expected {expected}, got {actual}")]
pub struct TypeMismatch {
    pub module_id: crate::ModuleId,
    pub expected: Type,
    pub actual: Type,
    pub span: crate::Span,
}

impl crate::Diag for TypeMismatch {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("invalid operands for {op}: {lhs} and {rhs}")]
pub struct InvalidBinaryOperands {
    pub module_id: crate::ModuleId,
    pub op: ast::BinaryOp,
    pub lhs: Type,
    pub rhs: Type,
    pub span: crate::Span,
}

impl crate::Diag for InvalidBinaryOperands {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("invalid operand for {op}: {operand}")]
pub struct InvalidUnaryOperand {
    pub module_id: crate::ModuleId,
    pub op: ast::UnaryOp,
    pub operand: Type,
    pub span: crate::Span,
}

impl crate::Diag for InvalidUnaryOperand {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("comparison between disjoint types: {lhs} and {rhs}")]
pub struct DisjointEquality {
    pub module_id: crate::ModuleId,
    pub lhs: Type,
    pub rhs: Type,
    pub span: crate::Span,
}

impl crate::Diag for DisjointEquality {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }

    fn level(&self) -> crate::DiagLevel {
        crate::DiagLevel::Warning
    }
}

#[derive(Clone, Debug)]
pub enum TypeIssue {
    Mismatch(Type, Type),
}

impl std::fmt::Display for TypeIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeIssue::Mismatch(lhs, rhs) => write!(f, "{rhs} is not assignable to {lhs}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeError {
    issue: TypeIssue,
    cause: Option<Box<TypeError>>,
}

impl TypeError {
    pub fn new(issue: TypeIssue) -> Self {
        Self { issue, cause: None }
    }

    pub fn causing(self, issue: TypeIssue) -> Self {
        Self {
            issue,
            cause: Some(Box::new(self)),
        }
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.issue)?;
        if let Some(cause) = &self.cause {
            write!(f, ", because {cause}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TypeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause.as_ref() as &(dyn std::error::Error + 'static))
    }
}

#[derive(Error, Debug)]
#[error("invalid type: {error}")]
pub struct InvalidType {
    pub module_id: crate::ModuleId,
    pub error: TypeError,
    pub span: crate::Span,
}

impl crate::Diag for InvalidType {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("raise requires an exception, got {ty}")]
pub struct NotAnException {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for NotAnException {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("catch variable must be an exception or function returning an exception, got {ty}")]
pub struct InvalidCatchTarget {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for InvalidCatchTarget {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("catch argument provided but exception is not a function type")]
pub struct UnexpectedCatchArg {
    pub module_id: crate::ModuleId,
    pub span: crate::Span,
}

impl crate::Diag for UnexpectedCatchArg {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("wrong number of type arguments: expected {expected}, got {got}")]
pub struct WrongTypeArgCount {
    pub module_id: crate::ModuleId,
    pub expected: usize,
    pub got: usize,
    pub span: crate::Span,
}

impl crate::Diag for WrongTypeArgCount {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("missing type arguments: expected {expected}")]
pub struct MissingTypeArgs {
    pub module_id: crate::ModuleId,
    pub expected: usize,
    pub span: crate::Span,
}

impl crate::Diag for MissingTypeArgs {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("type arguments provided to non-generic function")]
pub struct UnexpectedTypeArgs {
    pub module_id: crate::ModuleId,
    pub span: crate::Span,
}

impl crate::Diag for UnexpectedTypeArgs {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("type argument {actual} does not satisfy bound {bound}")]
pub struct TypeArgBoundViolation {
    pub module_id: crate::ModuleId,
    pub actual: Type,
    pub bound: Type,
    pub span: crate::Span,
}

impl crate::Diag for TypeArgBoundViolation {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("unknown type: {name}")]
pub struct UnknownType {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub span: crate::Span,
}

impl crate::Diag for UnknownType {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("unknown field \"{name}\" in record literal")]
pub struct UnknownField {
    pub module_id: crate::ModuleId,
    pub name: String,
    pub span: crate::Span,
}

impl crate::Diag for UnknownField {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
pub enum TypeCheckError {
    #[error("module id missing during type checking")]
    ModuleIdMissing,
}

impl<'p, S: crate::SourceRepo> TypeChecker<'p, S> {
    fn types_disjoint(&self, lhs: &Type, rhs: &Type) -> bool {
        self.assign_type(lhs, rhs).is_err() && self.assign_type(rhs, lhs).is_err()
    }

    fn assign_type(&self, lhs: &Type, rhs: &Type) -> Result<(), TypeError> {
        self.assign_type_with_bounds(lhs, rhs, &HashMap::new())
    }

    fn assign_type_with_bounds(
        &self,
        lhs: &Type,
        rhs: &Type,
        bounds: &HashMap<usize, Type>,
    ) -> Result<(), TypeError> {
        // Unfold iso-recursive types to expose their underlying structure.
        // This handles cases like µA.{a: Int} being assigned to {a: Int}.
        let lhs = &lhs.unfold();
        let rhs = &rhs.unfold();

        if lhs == rhs || matches!(lhs.kind, TypeKind::Any) || matches!(rhs.kind, TypeKind::Never) {
            return Ok(());
        }

        if let TypeKind::Optional(lhs_inner) = &lhs.kind {
            return match &rhs.kind {
                TypeKind::Optional(rhs_inner) => self
                    .assign_type_with_bounds(lhs_inner.as_ref(), rhs_inner.as_ref(), bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
                // If rhs is a bounded type variable whose bound is Optional, resolve
                // the bound first so both sides can be unwrapped together. Otherwise,
                // fall through to the `_` branch which checks `inner assignable from rhs`.
                TypeKind::Var(id)
                    if bounds
                        .get(id)
                        .is_some_and(|b| matches!(b.kind, TypeKind::Optional(_))) =>
                {
                    let upper_bound = &bounds[id];
                    self.assign_type_with_bounds(lhs, upper_bound, bounds)
                        .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone())))
                }
                _ => self
                    .assign_type_with_bounds(lhs_inner.as_ref(), rhs, bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
            };
        }

        // If rhs is a bounded type variable, check assignability via its upper bound.
        if let TypeKind::Var(id) = rhs.kind
            && let Some(upper_bound) = bounds.get(&id)
        {
            return self
                .assign_type_with_bounds(lhs, upper_bound, bounds)
                .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone())));
        }

        match &lhs.kind {
            TypeKind::Record(lhs_record) => match &rhs.kind {
                TypeKind::Record(rhs_record) => {
                    for (name, lhs_field) in lhs_record.iter() {
                        let Some(rhs_field) = rhs_record.get(name) else {
                            if matches!(lhs_field.kind, TypeKind::Optional(_)) {
                                continue;
                            }
                            return Err(TypeError::new(TypeIssue::Mismatch(
                                lhs.clone(),
                                rhs.clone(),
                            )));
                        };
                        self.assign_type_with_bounds(lhs_field, rhs_field, bounds)
                            .map_err(|err| {
                                err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))
                            })?;
                    }
                    Ok(())
                }
                _ => Err(TypeError::new(TypeIssue::Mismatch(
                    lhs.clone(),
                    rhs.clone(),
                ))),
            },
            TypeKind::Dict(lhs_dict) => match &rhs.kind {
                TypeKind::Dict(rhs_dict) => {
                    self.assign_type_with_bounds(
                        lhs_dict.key.as_ref(),
                        rhs_dict.key.as_ref(),
                        bounds,
                    )
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone())))?;
                    self.assign_type_with_bounds(
                        lhs_dict.value.as_ref(),
                        rhs_dict.value.as_ref(),
                        bounds,
                    )
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone())))?;
                    Ok(())
                }
                _ => Err(TypeError::new(TypeIssue::Mismatch(
                    lhs.clone(),
                    rhs.clone(),
                ))),
            },
            TypeKind::List(lhs_inner) => match &rhs.kind {
                TypeKind::List(rhs_inner) => self
                    .assign_type_with_bounds(lhs_inner.as_ref(), rhs_inner.as_ref(), bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
                _ => Err(TypeError::new(TypeIssue::Mismatch(
                    lhs.clone(),
                    rhs.clone(),
                ))),
            },
            TypeKind::Fn(lhs_fn) => match &rhs.kind {
                TypeKind::Fn(rhs_fn) => self
                    .assign_fn_type(lhs_fn, rhs_fn, bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
                _ => Err(TypeError::new(TypeIssue::Mismatch(
                    lhs.clone(),
                    rhs.clone(),
                ))),
            },
            _ => Err(TypeError::new(TypeIssue::Mismatch(
                lhs.clone(),
                rhs.clone(),
            ))),
        }
    }

    /// Check that a function type `rhs` is assignable to `lhs`.
    ///
    /// Handles three cases:
    /// 1. Both non-generic: direct structural check with contravariant params
    /// 2. Generic rhs, non-generic lhs: unify to solve type params
    /// 3. Both generic: F-sub rule with contravariant bounds and alpha-renaming
    fn assign_fn_type(
        &self,
        lhs_fn: &FnType,
        rhs_fn: &FnType,
        bounds: &HashMap<usize, Type>,
    ) -> Result<(), TypeError> {
        if lhs_fn.params.len() != rhs_fn.params.len() {
            return Err(TypeError::new(TypeIssue::Mismatch(
                Type::Fn(lhs_fn.clone()),
                Type::Fn(rhs_fn.clone()),
            )));
        }

        match (lhs_fn.type_params.is_empty(), rhs_fn.type_params.is_empty()) {
            // Both non-generic: structural check with contravariant params
            (true, true) => {
                for (lhs_param, rhs_param) in lhs_fn.params.iter().zip(rhs_fn.params.iter()) {
                    // Contravariant: rhs_param <: lhs_param
                    self.assign_type_with_bounds(rhs_param, lhs_param, bounds)?;
                }
                // Covariant return
                self.assign_type_with_bounds(lhs_fn.ret.as_ref(), rhs_fn.ret.as_ref(), bounds)?;
                Ok(())
            }

            // Non-generic lhs, generic rhs: unify to solve rhs's type params
            (true, false) => self.unify_generic_fn(lhs_fn, rhs_fn, bounds),

            // Generic lhs, non-generic rhs: the rhs (a concrete fn) must be
            // assignable to any valid instantiation of lhs. This means we need
            // to check that for *every* valid instantiation of lhs's type params,
            // rhs is assignable. In practice, we check with the upper bounds.
            (false, true) => {
                // Instantiate lhs at its upper bounds
                let replacements: Vec<(usize, Type)> = lhs_fn
                    .type_params
                    .iter()
                    .map(|(id, bound)| (*id, bound.clone()))
                    .collect();
                let instantiated_lhs = FnType {
                    type_params: vec![],
                    params: lhs_fn
                        .params
                        .iter()
                        .map(|p| p.substitute(&replacements))
                        .collect(),
                    ret: Box::new(lhs_fn.ret.substitute(&replacements)),
                };
                self.assign_fn_type(&instantiated_lhs, rhs_fn, bounds)
            }

            // Both generic: F-sub rule
            // ∀(S <: U).A <: ∀(T <: V).B requires:
            // 1. Same number of type params
            // 2. V <: U (contravariant in bounds)
            // 3. A[S:=T] <: B (with T having bound V)
            (false, false) => {
                if lhs_fn.type_params.len() != rhs_fn.type_params.len() {
                    return Err(TypeError::new(TypeIssue::Mismatch(
                        Type::Fn(lhs_fn.clone()),
                        Type::Fn(rhs_fn.clone()),
                    )));
                }

                // Check bound contravariance: rhs bounds <: lhs bounds
                for ((_, lhs_bound), (_, rhs_bound)) in
                    lhs_fn.type_params.iter().zip(rhs_fn.type_params.iter())
                {
                    self.assign_type_with_bounds(lhs_bound, rhs_bound, bounds)?;
                }

                // Alpha-rename: substitute lhs type vars with rhs type vars
                let alpha_rename: Vec<(usize, Type)> = lhs_fn
                    .type_params
                    .iter()
                    .zip(rhs_fn.type_params.iter())
                    .map(|((lhs_id, _), (rhs_id, _))| (*lhs_id, Type::Var(*rhs_id)))
                    .collect();

                let renamed_lhs = FnType {
                    type_params: vec![],
                    params: lhs_fn
                        .params
                        .iter()
                        .map(|p| p.substitute(&alpha_rename))
                        .collect(),
                    ret: Box::new(lhs_fn.ret.substitute(&alpha_rename)),
                };
                let body_rhs = FnType {
                    type_params: vec![],
                    params: rhs_fn.params.clone(),
                    ret: rhs_fn.ret.clone(),
                };

                // Extend bounds with rhs type var bounds for the body check
                let mut extended_bounds = bounds.clone();
                for (id, bound) in &rhs_fn.type_params {
                    extended_bounds.insert(*id, bound.clone());
                }

                self.assign_fn_type(&renamed_lhs, &body_rhs, &extended_bounds)
            }
        }
    }

    /// Unification for assigning a concrete function type (lhs) to a generic function type (rhs).
    /// Walks the types structurally, collecting upper/lower bounds for rhs's free type variables,
    /// then verifies that all bounds converge (lower <: upper).
    fn unify_generic_fn(
        &self,
        lhs_fn: &FnType,
        rhs_fn: &FnType,
        bounds: &HashMap<usize, Type>,
    ) -> Result<(), TypeError> {
        let free_vars: HashSet<usize> = rhs_fn.type_params.iter().map(|(id, _)| *id).collect();

        // Initialize assertions from declared bounds
        let mut assertions: HashMap<usize, (Type, Type)> = rhs_fn
            .type_params
            .iter()
            .map(|(id, upper_bound)| (*id, (Type::Never, upper_bound.clone())))
            .collect();

        // Collect bounds from parameters (contravariant position)
        for (lhs_param, rhs_param) in lhs_fn.params.iter().zip(rhs_fn.params.iter()) {
            self.collect_bounds(
                lhs_param,
                rhs_param,
                Variance::Contravariant,
                &free_vars,
                &mut assertions,
            )?;
        }

        // Collect bounds from return type (covariant position)
        self.collect_bounds(
            lhs_fn.ret.as_ref(),
            rhs_fn.ret.as_ref(),
            Variance::Covariant,
            &free_vars,
            &mut assertions,
        )?;

        // Verify: for each type param, lower <: upper
        for (lower, upper) in assertions.values() {
            self.assign_type_with_bounds(upper, lower, bounds)
                .map_err(|err| {
                    err.causing(TypeIssue::Mismatch(
                        Type::Fn(lhs_fn.clone()),
                        Type::Fn(rhs_fn.clone()),
                    ))
                })?;
        }

        Ok(())
    }

    /// Walk two types structurally, collecting bounds for free type variables in rhs.
    fn collect_bounds(
        &self,
        lhs: &Type,
        rhs: &Type,
        variance: Variance,
        free_vars: &HashSet<usize>,
        assertions: &mut HashMap<usize, (Type, Type)>,
    ) -> Result<(), TypeError> {
        // If rhs is a free type variable, record the bound from lhs
        if let TypeKind::Var(id) = rhs.kind
            && free_vars.contains(&id)
        {
            let entry = assertions.get_mut(&id).expect("free var must have entry");
            match variance {
                Variance::Covariant => {
                    // lhs is an upper bound for this variable
                    self.tighten_upper(&mut entry.1, lhs)?;
                }
                Variance::Contravariant => {
                    // lhs is a lower bound for this variable
                    self.tighten_lower(&mut entry.0, lhs)?;
                }
            }
            return Ok(());
        }

        // Structural recursion for matching type constructors
        match (&lhs.kind, &rhs.kind) {
            (TypeKind::Optional(lhs_inner), TypeKind::Optional(rhs_inner)) => {
                self.collect_bounds(lhs_inner, rhs_inner, variance, free_vars, assertions)
            }
            (_, TypeKind::Optional(rhs_inner)) if variance == Variance::Covariant => {
                // Non-optional lhs can be assigned to optional rhs in covariant position
                self.collect_bounds(lhs, rhs_inner, variance, free_vars, assertions)
            }
            (TypeKind::List(lhs_inner), TypeKind::List(rhs_inner)) => {
                self.collect_bounds(lhs_inner, rhs_inner, variance, free_vars, assertions)
            }
            (TypeKind::Record(lhs_record), TypeKind::Record(rhs_record)) => {
                for (name, rhs_field) in rhs_record.iter() {
                    if let Some(lhs_field) = lhs_record.get(name) {
                        self.collect_bounds(lhs_field, rhs_field, variance, free_vars, assertions)?;
                    }
                }
                Ok(())
            }
            (TypeKind::Dict(lhs_dict), TypeKind::Dict(rhs_dict)) => {
                self.collect_bounds(
                    lhs_dict.key.as_ref(),
                    rhs_dict.key.as_ref(),
                    variance,
                    free_vars,
                    assertions,
                )?;
                self.collect_bounds(
                    lhs_dict.value.as_ref(),
                    rhs_dict.value.as_ref(),
                    variance,
                    free_vars,
                    assertions,
                )
            }
            (TypeKind::Fn(lhs_fn), TypeKind::Fn(rhs_fn))
                if lhs_fn.params.len() == rhs_fn.params.len() =>
            {
                // Parameters: flip variance
                let flipped = variance.flip();
                for (lhs_param, rhs_param) in lhs_fn.params.iter().zip(rhs_fn.params.iter()) {
                    self.collect_bounds(lhs_param, rhs_param, flipped, free_vars, assertions)?;
                }
                // Return: same variance
                self.collect_bounds(
                    lhs_fn.ret.as_ref(),
                    rhs_fn.ret.as_ref(),
                    variance,
                    free_vars,
                    assertions,
                )
            }
            _ => {
                // No structural match — just check assignability in the appropriate direction.
                // If there are no free vars in rhs, this is a plain type compatibility check.
                match variance {
                    Variance::Covariant => {
                        // lhs :> rhs (lhs is supertype)
                        self.assign_type(lhs, rhs).map_err(|err| {
                            err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))
                        })
                    }
                    Variance::Contravariant => {
                        // rhs :> lhs (rhs is supertype, i.e. lhs <: rhs)
                        self.assign_type(rhs, lhs).map_err(|err| {
                            err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))
                        })
                    }
                }
            }
        }
    }

    /// Tighten an upper bound: new bound must be a subtype of or equal to current.
    fn tighten_upper(&self, current: &mut Type, new_bound: &Type) -> Result<(), TypeError> {
        if self.assign_type(current, new_bound).is_ok() {
            // new_bound <: current, so new_bound is tighter — use it
            *current = new_bound.clone();
        } else if self.assign_type(new_bound, current).is_ok() {
            // current <: new_bound, current is already tighter — keep it
        } else {
            // Neither is a subtype of the other
            return Err(TypeError::new(TypeIssue::Mismatch(
                current.clone(),
                new_bound.clone(),
            )));
        }
        Ok(())
    }

    /// Tighten a lower bound: new bound must be a supertype of or equal to current.
    fn tighten_lower(&self, current: &mut Type, new_bound: &Type) -> Result<(), TypeError> {
        if self.assign_type(new_bound, current).is_ok() {
            // current <: new_bound, so new_bound is tighter (higher lower bound) — use it
            *current = new_bound.clone();
        } else if self.assign_type(current, new_bound).is_ok() {
            // new_bound <: current, current is already tighter — keep it
        } else {
            return Err(TypeError::new(TypeIssue::Mismatch(
                current.clone(),
                new_bound.clone(),
            )));
        }
        Ok(())
    }

    fn apply_expected_type(
        &self,
        env: &TypeEnv<'_>,
        span: crate::Span,
        ty: Type,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        if let Some(expected_type) = expected_type {
            // If the actual type is a free variable, constrain it to the
            // expected type rather than reporting an error. This allows
            // expressions like `if (b) 123 else test(false)` to propagate
            // the concrete branch type into the free-variable branch.
            if let TypeKind::Var(id) = &ty.kind
                && env.is_free_var(*id)
            {
                if let Some(fv) = &env.free_vars {
                    fv.borrow_mut().constrain(*id, expected_type.clone());
                }
            } else if let Err(error) =
                self.assign_type_with_bounds(expected_type, &ty, &env.maps.type_var_bounds)
            {
                diags.push(InvalidType {
                    module_id: env.module_id()?,
                    error,
                    span,
                });
            }
        }

        Ok(Diagnosed::new(ty, diags))
    }

    pub fn new(program: &'p Program<S>) -> Self {
        Self {
            program,
            global_cache: RefCell::new(HashMap::new()),
            import_cache: RefCell::new(HashMap::new()),
            type_level_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn check_program(&self) -> Result<Diagnosed<()>, TypeCheckError> {
        let env = TypeEnv::new();
        let mut diags = DiagList::new();

        for (_, package) in self.program.packages() {
            self.check_package(&env, package)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    pub fn check_package(
        &self,
        env: &TypeEnv<'_>,
        package: &Package<AnySource<S>>,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let package_id = package.package_id();
        let mut diags = DiagList::new();

        for (path, file_mod) in package.modules() {
            let module_id = module_id_for_path(&package_id, path);
            let env = env.with_module_id(&module_id);
            let _ = self.check_file_mod(&env, file_mod)?.unpack(&mut diags);
        }

        Ok(Diagnosed::new((), diags))
    }

    #[inline(never)]
    pub fn check_file_mod(
        &self,
        env: &TypeEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let cache_key = file_mod as *const ast::FileMod;
        if let Some(cached) = self.import_cache.borrow().get(&cache_key) {
            return Ok(Diagnosed::new(cached.clone(), DiagList::new()));
        }
        let globals = file_mod.find_globals();
        let imports = self.find_imports(file_mod);
        let mut env = env.with_globals(&globals).with_imports(&imports);

        let mut diags = DiagList::new();

        self.build_module_type_env(&mut env, file_mod, &mut diags)?;

        let mut exports = RecordType::default();

        for statement in &file_mod.statements {
            if let Some((name, ty)) = self.check_stmt(&env, statement)?.unpack(&mut diags) {
                exports.insert(name, ty);
            }
        }

        let result = Type::Record(exports);
        self.import_cache
            .borrow_mut()
            .insert(cache_key, result.clone());
        Ok(Diagnosed::new(result, diags))
    }

    /// Compute the type-level exports of a module (from `export type` declarations).
    #[inline(never)]
    pub fn type_level_exports(
        &self,
        env: &TypeEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Diagnosed<RecordType> {
        let cache_key = file_mod as *const ast::FileMod;
        if let Some(cached) = self.type_level_cache.borrow().get(&cache_key) {
            return Diagnosed::new(cached.clone(), DiagList::new());
        }

        let mut diags = DiagList::new();
        let mut type_exports = RecordType::default();

        let globals = file_mod.find_globals();
        let imports = self.find_imports(file_mod);
        let mut inner_env = env.with_globals(&globals).with_imports(&imports);

        // Reuse the shared helper — this also populates import type-level,
        // which is needed for transitive type re-exports (e.g., `export type Alias B.SomeType`).
        if let Err(_err) = self.build_module_type_env(&mut inner_env, file_mod, &mut diags) {
            return Diagnosed::new(type_exports, diags);
        }

        for statement in &file_mod.statements {
            if let ast::ModStmt::ExportTypeDef(type_def) = statement
                && let Some(ty) = inner_env.lookup_type_level(type_def.var.name.as_str())
            {
                type_exports.insert(type_def.var.name.clone(), ty.clone());
            }
        }

        self.type_level_cache
            .borrow_mut()
            .insert(cache_key, type_exports.clone());
        Diagnosed::new(type_exports, diags)
    }

    /// Build the type-level environment for a module:
    /// 1. Populate import type-level bindings.
    /// 2. Two-pass resolve of local type defs (first pass: collect names, second pass: resolve bodies).
    fn build_module_type_env(
        &self,
        env: &mut TypeEnv<'_>,
        file_mod: &ast::FileMod,
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        // Populate type-level namespace from imports first, so type defs can reference imported types.
        self.populate_import_type_level(env, file_mod, diags)?;

        let type_defs = file_mod.find_type_defs();

        // Pass 1: Register all type def names with placeholder types (Never).
        // This allows forward references between type defs in the same module.
        for type_def in &type_defs {
            *env = env.with_type_level(type_def.var.name.clone(), Type::Never);
        }

        // Pass 2: Resolve the bodies. Run two iterations so that forward references
        // (type B A / type A Int) resolve correctly — the first iteration resolves
        // definitions whose dependencies are already resolved, and the second iteration
        // picks up any that referenced not-yet-resolved definitions.
        for _ in 0..2 {
            for type_def in &type_defs {
                let resolved_ty = self.resolve_type_def(env, type_def).unpack(diags);
                *env = env.with_type_level(type_def.var.name.clone(), resolved_ty);
            }
        }

        Ok(())
    }

    #[inline(never)]
    pub fn check_stmt(
        &self,
        env: &TypeEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Diagnosed<Option<(String, Type)>>, TypeCheckError> {
        match stmt {
            ast::ModStmt::Import(import_stmt) => {
                let alias = import_stmt
                    .as_ref()
                    .vars
                    .last()
                    .expect("import path contains at least one segment");
                if let Some((cursor, _)) = &alias.cursor
                    && let Some((target_module_id, Some(import_file_mod))) =
                        env.lookup_import(alias.name.as_str())
                {
                    let cache_key = import_file_mod as *const ast::FileMod;
                    let imported_ty =
                        if let Some(cached) = self.import_cache.borrow().get(&cache_key) {
                            Some(cached.clone())
                        } else {
                            let import_env = TypeEnv::new().with_module_id(&target_module_id);
                            self.check_file_mod(&import_env, import_file_mod)
                                .ok()
                                .map(|d| {
                                    let ty = d.into_inner();
                                    self.import_cache.borrow_mut().insert(cache_key, ty.clone());
                                    ty
                                })
                        };
                    if let Some(ty) = imported_ty {
                        cursor.set_type(ty);
                    }
                }
                Ok(Diagnosed::new(None, DiagList::new()))
            }
            ast::ModStmt::Expr(expr) => {
                let mut diags = DiagList::new();
                self.check_expr(env, expr, None)?.unpack(&mut diags);
                Ok(Diagnosed::new(None, diags))
            }
            ast::ModStmt::Let(let_bind) => {
                let mut diags = DiagList::new();
                self.check_global_let_bind(env, let_bind)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(None, diags))
            }
            ast::ModStmt::Export(let_bind) => {
                let mut diags = DiagList::new();
                let ty = self
                    .check_global_let_bind(env, let_bind)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(Some((let_bind.var.name.clone(), ty)), diags))
            }
            // Type defs are resolved during check_file_mod setup; nothing to do here.
            ast::ModStmt::TypeDef(_) | ast::ModStmt::ExportTypeDef(_) => {
                Ok(Diagnosed::new(None, DiagList::new()))
            }
        }
    }

    #[inline(never)]
    pub fn check_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match expr.as_ref() {
            ast::Expr::Int(_) => {
                self.apply_expected_type(env, expr.span(), Type::Int, expected_type)
            }
            ast::Expr::Float(_) => {
                self.apply_expected_type(env, expr.span(), Type::Float, expected_type)
            }
            ast::Expr::Bool(_) => {
                self.apply_expected_type(env, expr.span(), Type::Bool, expected_type)
            }
            ast::Expr::Nil => self.apply_expected_type(
                env,
                expr.span(),
                Type::Optional(Box::new(Type::Never)),
                expected_type,
            ),
            ast::Expr::Str(_) => {
                self.apply_expected_type(env, expr.span(), Type::Str, expected_type)
            }
            ast::Expr::Extern(extern_expr) => {
                let mut diags = DiagList::new();
                let resolved_ty = self
                    .resolve_type_expr(env, &extern_expr.ty)
                    .unpack(&mut diags);
                let ty = self
                    .apply_expected_type(env, expr.span(), resolved_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::If(if_expr) => {
                let mut diags = DiagList::new();
                let bool_ty = Type::Bool;
                self.check_expr(env, if_expr.condition.as_ref(), Some(&bool_ty))?
                    .unpack(&mut diags);

                let then_ty = self
                    .check_expr(env, if_expr.then_expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                if let Some(else_expr) = if_expr.else_expr.as_ref() {
                    self.check_expr(env, else_expr.as_ref(), Some(&then_ty))?
                        .unpack(&mut diags);
                    let ty = self
                        .apply_expected_type(env, expr.span(), then_ty, expected_type)?
                        .unpack(&mut diags);
                    return Ok(Diagnosed::new(ty, diags));
                }

                let ty = self
                    .apply_expected_type(
                        env,
                        expr.span(),
                        Type::Optional(Box::new(then_ty)),
                        expected_type,
                    )?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Let(let_expr) => {
                let mut diags = DiagList::new();
                let annotation_ty = let_expr
                    .bind
                    .ty
                    .as_ref()
                    .map(|te| self.resolve_type_expr(env, te).unpack(&mut diags));
                let bind_ty = self
                    .check_expr(env, let_expr.bind.expr.as_ref(), annotation_ty.as_ref())?
                    .unpack(&mut diags);
                let bind_ty = annotation_ty.unwrap_or(bind_ty);
                let inner_env = env.with_local(
                    let_expr.bind.var.name.as_str(),
                    let_expr.bind.var.span(),
                    bind_ty,
                );
                let body_ty = self
                    .check_expr(&inner_env, let_expr.expr.as_ref(), expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(body_ty, diags))
            }
            ast::Expr::Fn(fn_expr) => self.check_fn_expr(env, expr, fn_expr, expected_type),
            ast::Expr::Call(call_expr) => self.check_call_expr(env, expr, call_expr, expected_type),
            ast::Expr::Unary(unary_expr) => {
                let mut diags = DiagList::new();
                let operand_ty = self
                    .check_expr(env, unary_expr.expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();

                let result_ty = if matches!(operand_ty.kind, TypeKind::Never) {
                    Type::Never
                } else {
                    match unary_expr.op {
                        ast::UnaryOp::Negate => match &operand_ty.kind {
                            TypeKind::Int => Type::Int,
                            TypeKind::Float => Type::Float,
                            _ => {
                                diags.push(InvalidUnaryOperand {
                                    module_id: env.module_id()?,
                                    op: unary_expr.op,
                                    operand: operand_ty.clone(),
                                    span: expr.span(),
                                });
                                Type::Never
                            }
                        },
                    }
                };

                let ty = self
                    .apply_expected_type(env, expr.span(), result_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Binary(binary_expr) => {
                let mut diags = DiagList::new();
                let lhs_ty = self
                    .check_expr(env, binary_expr.lhs.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                let rhs_ty = self
                    .check_expr(env, binary_expr.rhs.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();

                let result_ty =
                    if matches!(lhs_ty.kind, TypeKind::Never)
                        || matches!(rhs_ty.kind, TypeKind::Never)
                    {
                        Type::Never
                    } else {
                        match binary_expr.op {
                            ast::BinaryOp::Add => match (&lhs_ty.kind, &rhs_ty.kind) {
                                (TypeKind::Int, TypeKind::Int) => Type::Int,
                                (TypeKind::Float, TypeKind::Float) => Type::Float,
                                (TypeKind::Int, TypeKind::Float)
                                | (TypeKind::Float, TypeKind::Int) => Type::Float,
                                (TypeKind::Str, TypeKind::Str) => Type::Str,
                                _ => {
                                    diags.push(InvalidBinaryOperands {
                                        module_id: env.module_id()?,
                                        op: binary_expr.op,
                                        lhs: lhs_ty.clone(),
                                        rhs: rhs_ty.clone(),
                                        span: expr.span(),
                                    });
                                    Type::Never
                                }
                            },
                            ast::BinaryOp::Sub => match (&lhs_ty.kind, &rhs_ty.kind) {
                                (TypeKind::Int, TypeKind::Int) => Type::Int,
                                (TypeKind::Float, TypeKind::Float) => Type::Float,
                                (TypeKind::Int, TypeKind::Float)
                                | (TypeKind::Float, TypeKind::Int) => Type::Float,
                                _ => {
                                    diags.push(InvalidBinaryOperands {
                                        module_id: env.module_id()?,
                                        op: binary_expr.op,
                                        lhs: lhs_ty.clone(),
                                        rhs: rhs_ty.clone(),
                                        span: expr.span(),
                                    });
                                    Type::Never
                                }
                            },
                            ast::BinaryOp::Mul => match (&lhs_ty.kind, &rhs_ty.kind) {
                                (TypeKind::Int, TypeKind::Int) => Type::Int,
                                (TypeKind::Float, TypeKind::Float) => Type::Float,
                                (TypeKind::Int, TypeKind::Float)
                                | (TypeKind::Float, TypeKind::Int) => Type::Float,
                                _ => {
                                    diags.push(InvalidBinaryOperands {
                                        module_id: env.module_id()?,
                                        op: binary_expr.op,
                                        lhs: lhs_ty.clone(),
                                        rhs: rhs_ty.clone(),
                                        span: expr.span(),
                                    });
                                    Type::Never
                                }
                            },
                            ast::BinaryOp::Div => match (&lhs_ty.kind, &rhs_ty.kind) {
                                (TypeKind::Int, TypeKind::Int) => Type::Int,
                                (TypeKind::Float, TypeKind::Float) => Type::Float,
                                (TypeKind::Int, TypeKind::Float)
                                | (TypeKind::Float, TypeKind::Int) => Type::Float,
                                _ => {
                                    diags.push(InvalidBinaryOperands {
                                        module_id: env.module_id()?,
                                        op: binary_expr.op,
                                        lhs: lhs_ty.clone(),
                                        rhs: rhs_ty.clone(),
                                        span: expr.span(),
                                    });
                                    Type::Never
                                }
                            },
                            ast::BinaryOp::Eq | ast::BinaryOp::Neq => {
                                if self.types_disjoint(&lhs_ty, &rhs_ty) {
                                    diags.push(DisjointEquality {
                                        module_id: env.module_id()?,
                                        lhs: lhs_ty.clone(),
                                        rhs: rhs_ty.clone(),
                                        span: expr.span(),
                                    });
                                }
                                Type::Bool
                            }
                            ast::BinaryOp::Lt
                            | ast::BinaryOp::Lte
                            | ast::BinaryOp::Gt
                            | ast::BinaryOp::Gte => match (&lhs_ty.kind, &rhs_ty.kind) {
                                (TypeKind::Int, TypeKind::Int)
                                | (TypeKind::Float, TypeKind::Float)
                                | (TypeKind::Int, TypeKind::Float)
                                | (TypeKind::Float, TypeKind::Int) => Type::Bool,
                                _ => {
                                    diags.push(InvalidBinaryOperands {
                                        module_id: env.module_id()?,
                                        op: binary_expr.op,
                                        lhs: lhs_ty.clone(),
                                        rhs: rhs_ty.clone(),
                                        span: expr.span(),
                                    });
                                    Type::Never
                                }
                            },
                            ast::BinaryOp::And | ast::BinaryOp::Or => {
                                match (&lhs_ty.kind, &rhs_ty.kind) {
                                    (TypeKind::Bool, TypeKind::Bool) => Type::Bool,
                                    _ => {
                                        diags.push(InvalidBinaryOperands {
                                            module_id: env.module_id()?,
                                            op: binary_expr.op,
                                            lhs: lhs_ty.clone(),
                                            rhs: rhs_ty.clone(),
                                            span: expr.span(),
                                        });
                                        Type::Never
                                    }
                                }
                            }
                        }
                    };

                let ty = self
                    .apply_expected_type(env, expr.span(), result_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Var(var) => self.check_var_expr(env, expr, var, expected_type),
            ast::Expr::Record(record_expr) => {
                let mut diags = DiagList::new();
                let mut record_ty = RecordType::default();
                let expected_record = match expected_type {
                    Some(Type {
                        kind: TypeKind::Record(record_ty),
                        ..
                    }) => Some(record_ty),
                    _ => None,
                };

                for field in &record_expr.fields {
                    if let Some((cursor, offset)) = &field.var.cursor
                        && let Some(expected_record) = expected_record
                    {
                        let prefix = &field.var.name[..*offset];
                        for (name, _) in expected_record.iter() {
                            if name.starts_with(prefix) {
                                cursor.add_completion_candidate(
                                    crate::CompletionCandidate::Member(name.clone()),
                                );
                            }
                        }
                    }
                    let expected_field_ty = expected_record
                        .and_then(|record_ty| record_ty.get(field.var.name.as_str()));
                    let field_ty = self
                        .check_expr(env, &field.expr, expected_field_ty)?
                        .unpack(&mut diags);
                    if let Some((cursor, _)) = &field.var.cursor {
                        cursor.set_type(field_ty.clone());
                    }
                    record_ty.insert(field.var.name.clone(), field_ty);
                }
                let ty = Type::Record(record_ty);
                if let Some(expected_record) = expected_record {
                    let missing_field = expected_record.iter().any(|(name, field_ty)| {
                        matches!(&ty.kind, TypeKind::Record(record) if record.get(name).is_none())
                            && !matches!(field_ty.kind, TypeKind::Optional(_))
                    });
                    if missing_field {
                        diags.push(InvalidType {
                            module_id: env.module_id()?,
                            error: TypeError::new(TypeIssue::Mismatch(
                                Type::Record(expected_record.clone()),
                                ty.clone(),
                            )),
                            span: expr.span(),
                        });
                    }
                    for field in &record_expr.fields {
                        if expected_record.get(field.var.name.as_str()).is_none() {
                            diags.push(UnknownField {
                                module_id: env.module_id()?,
                                name: field.var.name.clone(),
                                span: field.var.span(),
                            });
                        }
                    }
                    return Ok(Diagnosed::new(ty, diags));
                }
                let ty = self
                    .apply_expected_type(env, expr.span(), ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Dict(dict_expr) => {
                let mut diags = DiagList::new();
                let expected_dict = match expected_type {
                    Some(Type {
                        kind: TypeKind::Dict(dict_ty),
                        ..
                    }) => Some(dict_ty),
                    _ => None,
                };

                let dict_ty = if let Some(expected_dict) = expected_dict {
                    let expected_key = expected_dict.key.as_ref().clone().unfold();
                    let expected_value = expected_dict.value.as_ref().clone().unfold();
                    for entry in &dict_expr.entries {
                        self.check_expr(env, &entry.key, Some(&expected_key))?
                            .unpack(&mut diags);
                        self.check_expr(env, &entry.value, Some(&expected_value))?
                            .unpack(&mut diags);
                    }
                    Type::Dict(DictType {
                        key: Box::new(expected_key),
                        value: Box::new(expected_value),
                    })
                } else if let Some((first, rest)) = dict_expr.entries.split_first() {
                    let key_ty = self
                        .check_expr(env, &first.key, None)?
                        .unpack(&mut diags)
                        .unfold();
                    let value_ty = self
                        .check_expr(env, &first.value, None)?
                        .unpack(&mut diags)
                        .unfold();
                    for entry in rest {
                        self.check_expr(env, &entry.key, Some(&key_ty))?
                            .unpack(&mut diags);
                        self.check_expr(env, &entry.value, Some(&value_ty))?
                            .unpack(&mut diags);
                    }
                    Type::Dict(DictType {
                        key: Box::new(key_ty),
                        value: Box::new(value_ty),
                    })
                } else {
                    Type::Dict(DictType {
                        key: Box::new(Type::Never),
                        value: Box::new(Type::Never),
                    })
                };

                let ty = self
                    .apply_expected_type(env, expr.span(), dict_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::List(list_expr) => {
                let mut diags = DiagList::new();
                let list_ty = if let Some(Type {
                    kind: TypeKind::List(expected_item_ty),
                    ..
                }) = expected_type
                {
                    let expected_item_ty = expected_item_ty.as_ref().clone().unfold();
                    for item in &list_expr.items {
                        self.check_list_item(env, item, Some(&expected_item_ty))?
                            .unpack(&mut diags);
                    }
                    Type::List(Box::new(expected_item_ty))
                } else if let Some((first, rest)) = list_expr.items.split_first() {
                    let first_ty = self
                        .check_list_item(env, first, None)?
                        .unpack(&mut diags)
                        .unfold();
                    for item in rest {
                        self.check_list_item(env, item, Some(&first_ty))?
                            .unpack(&mut diags);
                    }
                    Type::List(Box::new(first_ty))
                } else {
                    Type::List(Box::new(Type::Never))
                };
                let ty = self
                    .apply_expected_type(env, expr.span(), list_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Interp(interp_expr) => {
                let mut diags = DiagList::new();
                for part in &interp_expr.parts {
                    self.check_expr(env, part, None)?.unpack(&mut diags);
                }
                let ty = self
                    .apply_expected_type(env, expr.span(), Type::Str, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::TypeCast(cast) => {
                let mut diags = DiagList::new();
                let target_ty = self.resolve_type_expr(env, &cast.ty).unpack(&mut diags);
                self.check_expr(env, &cast.expr, Some(&target_ty))?
                    .unpack(&mut diags);
                let ty = self
                    .apply_expected_type(env, expr.span(), target_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::PropertyAccess(property_access) => {
                let mut diags = DiagList::new();
                let raw_lhs_ty = self
                    .check_expr(env, property_access.expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                let lhs_ty = env.resolve_var_bound(&raw_lhs_ty).unfold();
                if matches!(lhs_ty.kind, TypeKind::Never) {
                    return Ok(Diagnosed::new(Type::Never, diags));
                }

                // If the LHS is a free type variable, constrain it to be a
                // record containing the accessed member.
                if let TypeKind::Var(lhs_var_id) = &raw_lhs_ty.kind
                    && env.is_free_var(*lhs_var_id)
                {
                    let member_id = next_type_id();
                    let member_var = Type::Var(member_id);
                    if let Some(fv) = &env.free_vars {
                        fv.borrow_mut().register(member_id);
                        let mut record = RecordType::default();
                        record.insert(property_access.property.name.clone(), member_var.clone());
                        fv.borrow_mut().constrain(*lhs_var_id, Type::Record(record));
                    }
                    if let Some((cursor, _)) = &property_access.property.cursor {
                        cursor.set_type(member_var.clone());
                    }
                    let ty = self
                        .apply_expected_type(env, expr.span(), member_var, expected_type)?
                        .unpack(&mut diags);
                    return Ok(Diagnosed::new(ty, diags));
                }

                if let Some((cursor, offset)) = &property_access.property.cursor {
                    let prefix = &property_access.property.name[..*offset];
                    if let TypeKind::Record(record_ty) = &lhs_ty.kind {
                        for (name, _) in record_ty.iter() {
                            if name.starts_with(prefix) {
                                cursor.add_completion_candidate(
                                    crate::CompletionCandidate::Member(name.clone()),
                                );
                            }
                        }
                    }
                }
                let member_ty = match &lhs_ty.kind {
                    TypeKind::Record(record_ty) => record_ty
                        .get(property_access.property.name.as_str())
                        .cloned(),
                    _ => None,
                };
                if let Some(member_ty) = member_ty {
                    if let Some((cursor, _)) = &property_access.property.cursor {
                        cursor.set_type(member_ty.clone());
                    }
                    let ty = self
                        .apply_expected_type(env, expr.span(), member_ty, expected_type)?
                        .unpack(&mut diags);
                    return Ok(Diagnosed::new(ty, diags));
                }

                diags.push(UndefinedMember {
                    module_id: env.module_id()?,
                    name: property_access.property.name.clone(),
                    ty: lhs_ty,
                    property: property_access.property.clone(),
                });
                Ok(Diagnosed::new(Type::Never, diags))
            }
            ast::Expr::IndexedAccess(indexed_access) => {
                let mut diags = DiagList::new();
                let container_ty = self
                    .check_expr(env, indexed_access.expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                let container_ty = env.resolve_var_bound(&container_ty).unfold();
                if matches!(container_ty.kind, TypeKind::Never) {
                    return Ok(Diagnosed::new(Type::Never, diags));
                }
                let result_ty = match &container_ty.kind {
                    TypeKind::Dict(dict_ty) => {
                        self.check_expr(
                            env,
                            indexed_access.index.as_ref(),
                            Some(dict_ty.key.as_ref()),
                        )?
                        .unpack(&mut diags);
                        Type::Optional(dict_ty.value.clone())
                    }
                    TypeKind::List(inner_ty) => {
                        self.check_expr(env, indexed_access.index.as_ref(), Some(&Type::Int))?
                            .unpack(&mut diags);
                        Type::Optional(inner_ty.clone())
                    }
                    _ => {
                        diags.push(InvalidIndexTarget {
                            module_id: env.module_id()?,
                            ty: container_ty,
                            span: expr.span(),
                        });
                        Type::Never
                    }
                };
                let ty = self
                    .apply_expected_type(env, expr.span(), result_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Exception(exception_expr) => {
                let mut diags = DiagList::new();
                let exception_ty = Type::Exception(exception_expr.exception_id);
                if let Some(ty_expr) = &exception_expr.ty {
                    let param_ty = self.resolve_type_expr(env, ty_expr).unpack(&mut diags);
                    let fn_ty = Type::Fn(FnType {
                        type_params: vec![],
                        params: vec![param_ty],
                        ret: Box::new(exception_ty),
                    });
                    let ty = self
                        .apply_expected_type(env, expr.span(), fn_ty, expected_type)?
                        .unpack(&mut diags);
                    Ok(Diagnosed::new(ty, diags))
                } else {
                    let ty = self
                        .apply_expected_type(env, expr.span(), exception_ty, expected_type)?
                        .unpack(&mut diags);
                    Ok(Diagnosed::new(ty, diags))
                }
            }
            ast::Expr::Raise(raise_expr) => {
                let mut diags = DiagList::new();
                let inner_ty = self
                    .check_expr(env, raise_expr.expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                if !matches!(inner_ty.kind, TypeKind::Exception(_) | TypeKind::Never) {
                    diags.push(NotAnException {
                        module_id: env.module_id()?,
                        ty: inner_ty,
                        span: raise_expr.expr.span(),
                    });
                }
                let ty = self
                    .apply_expected_type(env, expr.span(), Type::Never, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Try(try_expr) => {
                let mut diags = DiagList::new();
                let try_ty = self
                    .check_expr(env, try_expr.expr.as_ref(), expected_type)?
                    .unpack(&mut diags)
                    .unfold();

                for catch in &try_expr.catches {
                    let catch_var_ty = self
                        .check_expr(
                            env,
                            &crate::Loc::new(
                                ast::Expr::Var(catch.exception_var.clone()),
                                catch.exception_var.span(),
                            ),
                            None,
                        )?
                        .unpack(&mut diags)
                        .unfold();

                    match &catch_var_ty.kind {
                        TypeKind::Exception(_) => {
                            if let Some(catch_arg) = &catch.catch_arg {
                                diags.push(UnexpectedCatchArg {
                                    module_id: env.module_id()?,
                                    span: catch_arg.span(),
                                });
                            }
                            self.check_expr(env, &catch.body, Some(&try_ty))?
                                .unpack(&mut diags);
                        }
                        TypeKind::Fn(fn_ty) => {
                            let ret_ty = fn_ty.ret.as_ref().clone().unfold();
                            if !matches!(ret_ty.kind, TypeKind::Exception(_)) {
                                diags.push(InvalidCatchTarget {
                                    module_id: env.module_id()?,
                                    ty: catch_var_ty.clone(),
                                    span: catch.exception_var.span(),
                                });
                            }
                            if let Some(catch_arg) = &catch.catch_arg {
                                let param_ty = fn_ty.params.first().cloned().unwrap_or(Type::Never);
                                let inner_env = env.with_local(
                                    catch_arg.name.as_str(),
                                    catch_arg.span(),
                                    param_ty,
                                );
                                self.check_expr(&inner_env, &catch.body, Some(&try_ty))?
                                    .unpack(&mut diags);
                            } else {
                                self.check_expr(env, &catch.body, Some(&try_ty))?
                                    .unpack(&mut diags);
                            }
                        }
                        TypeKind::Never => {
                            // If the type is Never (e.g., undefined variable), skip further checks
                            self.check_expr(env, &catch.body, Some(&try_ty))?
                                .unpack(&mut diags);
                        }
                        _ => {
                            diags.push(InvalidCatchTarget {
                                module_id: env.module_id()?,
                                ty: catch_var_ty,
                                span: catch.exception_var.span(),
                            });
                            self.check_expr(env, &catch.body, Some(&try_ty))?
                                .unpack(&mut diags);
                        }
                    }
                }

                let ty = self
                    .apply_expected_type(env, expr.span(), try_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
        }
    }

    /// Check a function expression. Extracted from `check_expr` to reduce
    /// the match arm's contribution to the parent stack frame.
    #[inline(never)]
    fn check_fn_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        fn_expr: &ast::FnExpr,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let mut fn_env = env.inner();

        // Allocate type variable IDs for generic type parameters
        let mut type_param_entries = Vec::with_capacity(fn_expr.type_params.len());
        for type_param in &fn_expr.type_params {
            let type_id = next_type_id();
            fn_env = fn_env.with_type_var(type_param.var.name.clone(), Type::Var(type_id));
            let upper_bound = if let Some(bound_expr) = &type_param.bound {
                self.resolve_type_expr(&fn_env, bound_expr)
                    .unpack(&mut diags)
            } else {
                Type::Any
            };
            fn_env = fn_env.with_type_var_bound(type_id, upper_bound.clone());
            type_param_entries.push((type_id, upper_bound));
        }

        let mut params = Vec::with_capacity(fn_expr.params.len());
        for param in &fn_expr.params {
            let param_ty = self
                .resolve_type_expr(&fn_env, &param.ty)
                .unpack(&mut diags);
            fn_env = fn_env.with_local(param.var.name.as_str(), param.var.span(), param_ty.clone());
            params.push(param_ty);
        }

        let ret = self
            .check_expr(&fn_env, fn_expr.body.as_ref(), None)?
            .unpack(&mut diags);
        let ty = self
            .apply_expected_type(
                env,
                expr.span(),
                Type::Fn(FnType {
                    type_params: type_param_entries,
                    params,
                    ret: Box::new(ret),
                }),
                expected_type,
            )?
            .unpack(&mut diags);
        Ok(Diagnosed::new(ty, diags))
    }

    /// Check a call expression. Extracted from `check_expr` to reduce
    /// the match arm's contribution to the parent stack frame.
    #[inline(never)]
    fn check_call_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        call_expr: &ast::CallExpr,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let raw_callee_ty = self
            .check_expr(env, call_expr.callee.as_ref(), None)?
            .unpack(&mut diags)
            .unfold();
        let callee_ty = env.resolve_var_bound(&raw_callee_ty).unfold();
        if matches!(callee_ty.kind, TypeKind::Never) {
            return Ok(Diagnosed::new(Type::Never, diags));
        }

        // If the callee is a free type variable, constrain it to be a
        // function type derived from the arguments.
        if let TypeKind::Var(callee_var_id) = &raw_callee_ty.kind
            && env.is_free_var(*callee_var_id)
        {
            let mut arg_types = Vec::new();
            for arg in &call_expr.args {
                let arg_ty = self.check_expr(env, arg, None)?.unpack(&mut diags);
                arg_types.push(arg_ty);
            }
            let ret_id = next_type_id();
            let ret_var = Type::Var(ret_id);
            if let Some(fv) = &env.free_vars {
                fv.borrow_mut().register(ret_id);
                let fn_constraint = Type::Fn(FnType {
                    type_params: vec![],
                    params: arg_types,
                    ret: Box::new(ret_var.clone()),
                });
                fv.borrow_mut().constrain(*callee_var_id, fn_constraint);
            }
            let ty = self
                .apply_expected_type(env, expr.span(), ret_var, expected_type)?
                .unpack(&mut diags);
            return Ok(Diagnosed::new(ty, diags));
        }

        let TypeKind::Fn(fn_ty) = callee_ty.kind else {
            diags.push(NotAFunction {
                module_id: env.module_id()?,
                ty: callee_ty,
                span: call_expr.callee.span(),
            });
            return Ok(Diagnosed::new(Type::Never, diags));
        };

        // Handle type argument instantiation for generic functions
        let fn_ty = if !call_expr.type_args.is_empty() {
            if fn_ty.type_params.is_empty() {
                diags.push(UnexpectedTypeArgs {
                    module_id: env.module_id()?,
                    span: expr.span(),
                });
                fn_ty
            } else if call_expr.type_args.len() != fn_ty.type_params.len() {
                diags.push(WrongTypeArgCount {
                    module_id: env.module_id()?,
                    expected: fn_ty.type_params.len(),
                    got: call_expr.type_args.len(),
                    span: expr.span(),
                });
                // Substitute Any for params (to accept any argument) and Never for return
                // (to be usable anywhere). This prevents downstream type errors.
                let param_replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .map(|(id, _)| (*id, Type::Any))
                    .collect();
                let ret_replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .map(|(id, _)| (*id, Type::Never))
                    .collect();
                FnType {
                    type_params: vec![],
                    params: fn_ty
                        .params
                        .iter()
                        .map(|p| p.substitute(&param_replacements))
                        .collect(),
                    ret: Box::new(fn_ty.ret.substitute(&ret_replacements)),
                }
            } else {
                // Build substitution map from type param IDs to resolved type args
                // and check each type arg against its declared bound
                let replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .zip(call_expr.type_args.iter())
                    .map(|((id, bound), type_arg)| {
                        let resolved = self.resolve_type_expr(env, type_arg).unpack(&mut diags);
                        // Check that the type argument satisfies the declared bound
                        if self
                            .assign_type_with_bounds(bound, &resolved, &env.maps.type_var_bounds)
                            .is_err()
                        {
                            diags.push(TypeArgBoundViolation {
                                module_id: env.module_id().unwrap(),
                                actual: resolved.clone(),
                                bound: bound.clone(),
                                span: type_arg.span(),
                            });
                        }
                        (*id, resolved)
                    })
                    .collect();
                FnType {
                    type_params: vec![],
                    params: fn_ty
                        .params
                        .iter()
                        .map(|p| p.substitute(&replacements))
                        .collect(),
                    ret: Box::new(fn_ty.ret.substitute(&replacements)),
                }
            }
        } else if !fn_ty.type_params.is_empty() {
            // Generic function called without type arguments
            diags.push(MissingTypeArgs {
                module_id: env.module_id()?,
                expected: fn_ty.type_params.len(),
                span: call_expr.callee.span(),
            });
            // Substitute Any for params (to accept any argument) and Never for return
            // (to be usable anywhere). This prevents downstream type errors.
            let param_replacements: Vec<(usize, Type)> = fn_ty
                .type_params
                .iter()
                .map(|(id, _)| (*id, Type::Any))
                .collect();
            let ret_replacements: Vec<(usize, Type)> = fn_ty
                .type_params
                .iter()
                .map(|(id, _)| (*id, Type::Never))
                .collect();
            FnType {
                type_params: vec![],
                params: fn_ty
                    .params
                    .iter()
                    .map(|p| p.substitute(&param_replacements))
                    .collect(),
                ret: Box::new(fn_ty.ret.substitute(&ret_replacements)),
            }
        } else {
            fn_ty
        };

        if call_expr.args.len() < fn_ty.params.len() {
            diags.push(MissingArguments {
                module_id: env.module_id()?,
                expected: fn_ty.params.len(),
                got: call_expr.args.len(),
                span: call_expr.callee.span(),
            });
        }

        for (index, arg) in call_expr.args.iter().enumerate() {
            let Some(param_ty) = fn_ty.params.get(index) else {
                diags.push(ExtraneousArgument {
                    module_id: env.module_id()?,
                    index,
                    span: arg.span(),
                });
                continue;
            };

            self.check_expr(env, arg, Some(param_ty))?
                .unpack(&mut diags);
        }

        let ty = self
            .apply_expected_type(env, expr.span(), *fn_ty.ret, expected_type)?
            .unpack(&mut diags);
        Ok(Diagnosed::new(ty, diags))
    }

    /// Check a variable expression. Extracted from `check_expr` to reduce
    /// the match arm's contribution to the parent stack frame. Also handles
    /// caching of global and import type checks.
    #[inline(never)]
    fn check_var_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        var: &crate::Loc<ast::Var>,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        if let Some((cursor, offset)) = &var.cursor {
            let prefix = &var.name[..*offset];
            for name in env.local_names().chain(env.global_names()) {
                if name.starts_with(prefix) {
                    cursor
                        .add_completion_candidate(crate::CompletionCandidate::Var(name.to_owned()));
                }
            }
        }
        let set_cursor = |decl: crate::Span, ty: &Type| {
            if let Some((cursor, _)) = &var.cursor {
                cursor.set_declaration(decl);
                cursor.set_type(ty.clone());
            }
        };
        let track_ref = |decl: crate::Span| {
            if let Some(cursor) = &env.cursor {
                cursor.track_reference(decl, expr.span());
            }
        };
        if let Some((decl, local_ty)) = env.lookup_local(var.name.as_str()) {
            let decl = *decl;
            let local_ty = local_ty.clone();
            track_ref(decl);
            set_cursor(decl, &local_ty);
            return self.apply_expected_type(env, expr.span(), local_ty, expected_type);
        }
        if let Some((decl, global_expr)) = env.lookup_global(var.name.as_str()) {
            let mut diags = DiagList::new();
            let cache_key = global_expr as *const crate::Loc<ast::Expr>;
            let resolved_ty = if let Some(cached_ty) = self.global_cache.borrow().get(&cache_key) {
                cached_ty.clone()
            } else {
                let type_id = next_type_id();
                let constraints = Rc::new(RefCell::new(FreeVarConstraints::new()));
                let global_env = env.without_locals().with_free_var(
                    var.name.as_str(),
                    decl,
                    type_id,
                    constraints.clone(),
                );
                let resolved_ty = self
                    .check_expr(&global_env, global_expr, None)?
                    .unpack(&mut diags);
                // Solve free variable constraints and substitute into the body type
                let solved = constraints.borrow().solve(type_id, &resolved_ty);
                let resolved_ty = resolved_ty.substitute(&solved);
                self.global_cache
                    .borrow_mut()
                    .insert(cache_key, resolved_ty.clone());
                resolved_ty
            };
            let type_id = next_type_id();
            let ty = self
                .apply_expected_type(
                    env,
                    expr.span(),
                    Type::IsoRec(type_id, Box::new(resolved_ty)),
                    expected_type,
                )?
                .unpack(&mut diags);
            track_ref(decl);
            set_cursor(decl, &ty);
            return Ok(Diagnosed::new(ty, diags));
        }
        if let Some((target_module_id, maybe_import_file_mod)) =
            env.lookup_import(var.name.as_str())
        {
            let Some(import_file_mod) = maybe_import_file_mod else {
                return Ok(Diagnosed::new(Type::Never, DiagList::new()));
            };
            let cache_key = import_file_mod as *const ast::FileMod;
            let imported_ty = if let Some(cached_ty) = self.import_cache.borrow().get(&cache_key) {
                cached_ty.clone()
            } else {
                let import_env = TypeEnv::new().with_module_id(&target_module_id);
                let imported_ty = self.check_file_mod(&import_env, import_file_mod)?;
                let imported_ty = imported_ty.into_inner();
                self.import_cache
                    .borrow_mut()
                    .insert(cache_key, imported_ty.clone());
                imported_ty
            };
            if let Some((cursor, _)) = &var.cursor {
                cursor.set_type(imported_ty.clone());
            }
            return self.apply_expected_type(env, expr.span(), imported_ty, expected_type);
        }
        let mut diags = DiagList::new();
        diags.push(UndefinedVariable {
            module_id: env.module_id()?,
            name: var.name.clone(),
            var: var.clone(),
        });
        Ok(Diagnosed::new(Type::Never, diags))
    }

    fn check_list_item(
        &self,
        env: &TypeEnv<'_>,
        item: &ast::ListItem,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match item {
            ast::ListItem::Expr(expr) => self.check_expr(env, expr, expected_type),
            ast::ListItem::If(if_item) => {
                let mut diags = DiagList::new();
                let bool_ty = Type::Bool;
                self.check_expr(env, if_item.condition.as_ref(), Some(&bool_ty))?
                    .unpack(&mut diags);
                let item_ty = self
                    .check_list_item(env, if_item.then_item.as_ref(), expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(item_ty, diags))
            }
            ast::ListItem::For(for_item) => {
                let mut diags = DiagList::new();
                let iterable_ty = self
                    .check_expr(env, for_item.iterable.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                let element_ty = match iterable_ty.kind {
                    TypeKind::List(element_ty) => *element_ty,
                    _ => {
                        diags.push(InvalidType {
                            module_id: env.module_id()?,
                            error: TypeError::new(TypeIssue::Mismatch(
                                Type::List(Box::new(Type::Any)),
                                iterable_ty,
                            )),
                            span: for_item.iterable.span(),
                        });
                        Type::Never
                    }
                };
                let inner_env =
                    env.with_local(for_item.var.name.as_str(), for_item.var.span(), element_ty);
                let item_ty = self
                    .check_list_item(&inner_env, for_item.emit_item.as_ref(), expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(item_ty, diags))
            }
        }
    }

    #[inline(never)]
    pub fn check_global_let_bind(
        &self,
        env: &TypeEnv<'_>,
        let_bind: &ast::LetBind,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let annotation_ty = let_bind
            .ty
            .as_ref()
            .map(|te| self.resolve_type_expr(env, te).unpack(&mut diags));
        let type_id = next_type_id();
        let constraints = Rc::new(RefCell::new(FreeVarConstraints::new()));
        let env = env.with_free_var(
            let_bind.var.name.as_str(),
            let_bind.var.span(),
            type_id,
            constraints.clone(),
        );
        let resolved_ty = self
            .check_expr(&env, let_bind.expr.as_ref(), annotation_ty.as_ref())?
            .unpack(&mut diags);
        // Solve free variable constraints and substitute into the body type
        let solved = constraints.borrow().solve(type_id, &resolved_ty);
        let resolved_ty = resolved_ty.substitute(&solved);
        // Populate the global cache so subsequent references don't re-check.
        let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
        self.global_cache
            .borrow_mut()
            .insert(cache_key, resolved_ty.clone());
        let ty = if let Some(ann_ty) = annotation_ty {
            Type::IsoRec(type_id, Box::new(ann_ty))
        } else {
            Type::IsoRec(type_id, Box::new(resolved_ty))
        };
        if let Some((cursor, _)) = &let_bind.var.cursor {
            cursor.set_type(ty.clone());
        }
        Ok(Diagnosed::new(ty, diags))
    }

    fn resolve_type_expr(
        &self,
        env: &TypeEnv<'_>,
        type_expr: &crate::Loc<ast::TypeExpr>,
    ) -> Diagnosed<Type> {
        let mut diags = DiagList::new();
        let ty = match type_expr.as_ref() {
            ast::TypeExpr::Var(var) if var.name == "Any" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Any);
                }
                Type::Any
            }
            ast::TypeExpr::Var(var) if var.name == "Int" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Int);
                }
                Type::Int
            }
            ast::TypeExpr::Var(var) if var.name == "Float" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Float);
                }
                Type::Float
            }
            ast::TypeExpr::Var(var) if var.name == "Bool" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Bool);
                }
                Type::Bool
            }
            ast::TypeExpr::Var(var) if var.name == "Str" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Str);
                }
                Type::Str
            }
            ast::TypeExpr::Var(var) => {
                // Type variables (from fn<A>) take priority, then type-level bindings (from type defs/imports)
                let resolved = if let Some(ty) = env.lookup_type_var(var.name.as_str()) {
                    ty.clone()
                } else if let Some(ty) = env.lookup_type_level(var.name.as_str()) {
                    let ty = ty.clone();
                    // Attach the alias name for non-generic, non-primitive type aliases.
                    // Generic type constructors (Fn with type_params) get their name
                    // when instantiated via TypeExpr::Application.
                    if !matches!(ty.kind, TypeKind::Fn(ref f) if !f.type_params.is_empty()) {
                        ty.with_name(var.name.clone())
                    } else {
                        ty
                    }
                } else {
                    if let Ok(module_id) = env.module_id() {
                        diags.push(UnknownType {
                            module_id,
                            name: var.name.clone(),
                            span: type_expr.span(),
                        });
                    }
                    Type::Never
                };
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(resolved.clone());
                }
                resolved
            }
            ast::TypeExpr::Optional(inner) => {
                let inner_ty = self
                    .resolve_type_expr(env, inner.as_ref())
                    .unpack(&mut diags);
                Type::Optional(Box::new(inner_ty))
            }
            ast::TypeExpr::List(inner) => {
                let inner_ty = self
                    .resolve_type_expr(env, inner.as_ref())
                    .unpack(&mut diags);
                Type::List(Box::new(inner_ty))
            }
            ast::TypeExpr::Fn(fn_ty) => {
                let mut fn_env = env.inner();
                let mut type_param_entries = Vec::with_capacity(fn_ty.type_params.len());
                for type_param in &fn_ty.type_params {
                    let type_id = next_type_id();
                    fn_env = fn_env.with_type_var(type_param.var.name.clone(), Type::Var(type_id));
                    let upper_bound = if let Some(bound_expr) = &type_param.bound {
                        self.resolve_type_expr(&fn_env, bound_expr)
                            .unpack(&mut diags)
                    } else {
                        Type::Any
                    };
                    type_param_entries.push((type_id, upper_bound));
                }
                let params = fn_ty
                    .params
                    .iter()
                    .map(|param| self.resolve_type_expr(&fn_env, param).unpack(&mut diags))
                    .collect();
                let ret = self
                    .resolve_type_expr(&fn_env, &fn_ty.ret)
                    .unpack(&mut diags);
                Type::Fn(FnType {
                    type_params: type_param_entries,
                    params,
                    ret: Box::new(ret),
                })
            }
            ast::TypeExpr::Record(record_ty) => {
                let mut resolved = RecordType::default();
                for field in &record_ty.fields {
                    let field_ty = self.resolve_type_expr(env, &field.ty).unpack(&mut diags);
                    resolved.insert(field.var.name.clone(), field_ty);
                }
                Type::Record(resolved)
            }
            ast::TypeExpr::Dict(dict_ty) => {
                let key = self.resolve_type_expr(env, &dict_ty.key).unpack(&mut diags);
                let value = self
                    .resolve_type_expr(env, &dict_ty.value)
                    .unpack(&mut diags);
                Type::Dict(DictType {
                    key: Box::new(key),
                    value: Box::new(value),
                })
            }
            ast::TypeExpr::Application(app) => {
                let base_ty = self
                    .resolve_type_expr(env, app.base.as_ref())
                    .unpack(&mut diags);
                match &base_ty.kind {
                    TypeKind::Fn(fn_ty)
                        if fn_ty.params.is_empty() && !fn_ty.type_params.is_empty() =>
                    {
                        if fn_ty.type_params.len() != app.args.len() {
                            if let Ok(module_id) = env.module_id() {
                                diags.push(WrongTypeArgCount {
                                    module_id,
                                    expected: fn_ty.type_params.len(),
                                    got: app.args.len(),
                                    span: type_expr.span(),
                                });
                            }
                            Type::Never
                        } else {
                            let resolved_args: Vec<Type> = app
                                .args
                                .iter()
                                .map(|arg| self.resolve_type_expr(env, arg).unpack(&mut diags))
                                .collect();
                            // Check that each arg satisfies its bound
                            for ((id, bound), arg_ty) in
                                fn_ty.type_params.iter().zip(resolved_args.iter())
                            {
                                let instantiated_bound = bound.substitute(
                                    &fn_ty
                                        .type_params
                                        .iter()
                                        .zip(resolved_args.iter())
                                        .map(|((id, _), ty)| (*id, ty.clone()))
                                        .collect::<Vec<_>>(),
                                );
                                if self.assign_type(&instantiated_bound, arg_ty).is_err()
                                    && let Ok(module_id) = env.module_id()
                                {
                                    diags.push(TypeArgBoundViolation {
                                        module_id,
                                        actual: arg_ty.clone(),
                                        bound: instantiated_bound,
                                        span: type_expr.span(),
                                    });
                                }
                                let _ = id;
                            }
                            // Build the application name (e.g., "Pair<Int, Str>")
                            let app_name = {
                                let base_name = match &**app.base.as_ref() {
                                    ast::TypeExpr::Var(var) => Some(var.name.as_str()),
                                    _ => None,
                                };
                                base_name.map(|name| {
                                    let args_str: Vec<String> =
                                        resolved_args.iter().map(|ty| ty.to_string()).collect();
                                    format!("{name}<{}>", args_str.join(", "))
                                })
                            };
                            // Substitute type params with the provided args
                            let replacements: Vec<(usize, Type)> = fn_ty
                                .type_params
                                .iter()
                                .zip(resolved_args)
                                .map(|((id, _), ty)| (*id, ty))
                                .collect();
                            let result = fn_ty.ret.substitute(&replacements);
                            match app_name {
                                Some(name) => result.with_name(name),
                                None => result,
                            }
                        }
                    }
                    _ => {
                        if let Ok(module_id) = env.module_id() {
                            diags.push(UnexpectedTypeArgs {
                                module_id,
                                span: type_expr.span(),
                            });
                        }
                        Type::Never
                    }
                }
            }
            ast::TypeExpr::PropertyAccess(prop_access) => {
                let lhs_ty = self
                    .resolve_type_expr(env, prop_access.expr.as_ref())
                    .unpack(&mut diags);
                if let Some((cursor, offset)) = &prop_access.property.cursor {
                    let prefix = &prop_access.property.name[..*offset];
                    if let TypeKind::Record(record_ty) = &lhs_ty.kind {
                        for (name, _) in record_ty.iter() {
                            if name.starts_with(prefix) {
                                cursor.add_completion_candidate(
                                    crate::CompletionCandidate::Member(name.clone()),
                                );
                            }
                        }
                    }
                }
                match &lhs_ty.kind {
                    TypeKind::Record(record_ty) => {
                        if let Some(member_ty) = record_ty.get(prop_access.property.name.as_str()) {
                            if let Some((cursor, _)) = &prop_access.property.cursor {
                                cursor.set_type(member_ty.clone());
                            }
                            member_ty.clone()
                        } else {
                            if let Ok(module_id) = env.module_id() {
                                diags.push(UndefinedMember {
                                    module_id,
                                    name: prop_access.property.name.clone(),
                                    ty: lhs_ty,
                                    property: prop_access.property.clone(),
                                });
                            }
                            Type::Never
                        }
                    }
                    TypeKind::Never => Type::Never,
                    _ => {
                        if let Ok(module_id) = env.module_id() {
                            diags.push(UndefinedMember {
                                module_id,
                                name: prop_access.property.name.clone(),
                                ty: lhs_ty,
                                property: prop_access.property.clone(),
                            });
                        }
                        Type::Never
                    }
                }
            }
        };
        Diagnosed::new(ty, diags)
    }

    /// Resolve a type definition to its underlying `Type`.
    /// For generic type defs like `type Pair<A, B> { fst: A, snd: B }`,
    /// this produces a function type `fn<A, B>() { fst: A, snd: B }` that
    /// acts as a type-level constructor.
    pub fn resolve_type_def(&self, env: &TypeEnv<'_>, type_def: &ast::TypeDef) -> Diagnosed<Type> {
        let mut diags = DiagList::new();

        if type_def.type_params.is_empty() {
            // Simple type alias: `type Name Str`
            let ty = self.resolve_type_expr(env, &type_def.ty).unpack(&mut diags);
            return Diagnosed::new(ty, diags);
        }

        // Generic type def: `type Pair<A, B> { fst: A, snd: B }`
        // Resolve type params and produce a Fn type that acts as a type-level constructor.
        let mut fn_env = env.inner();
        let mut type_param_entries = Vec::with_capacity(type_def.type_params.len());
        for type_param in &type_def.type_params {
            let type_id = next_type_id();
            fn_env = fn_env.with_type_var(type_param.var.name.clone(), Type::Var(type_id));
            let upper_bound = if let Some(bound_expr) = &type_param.bound {
                self.resolve_type_expr(&fn_env, bound_expr)
                    .unpack(&mut diags)
            } else {
                Type::Any
            };
            fn_env = fn_env.with_type_var_bound(type_id, upper_bound.clone());
            type_param_entries.push((type_id, upper_bound));
        }

        let body_ty = self
            .resolve_type_expr(&fn_env, &type_def.ty)
            .unpack(&mut diags);

        // Represent as fn<A, B>() ReturnType — a zero-param generic function.
        // When used as a type with type arguments (e.g., Pair<Int, Str>), the type checker
        // will substitute the type params.
        Diagnosed::new(
            Type::Fn(FnType {
                type_params: type_param_entries,
                params: vec![],
                ret: Box::new(body_ty),
            }),
            diags,
        )
    }

    /// Populate the type-level namespace of `env` with type exports from imported modules.
    #[inline(never)]
    fn populate_import_type_level(
        &self,
        env: &mut TypeEnv<'_>,
        file_mod: &ast::FileMod,
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        for statement in &file_mod.statements {
            if let ast::ModStmt::Import(import_stmt) = statement {
                let alias = import_stmt
                    .as_ref()
                    .vars
                    .last()
                    .expect("import path contains at least one segment");

                if let Some(import_file_mod) = self.resolve_import(import_stmt) {
                    let target_module_id = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect::<crate::ModuleId>();
                    let import_env = TypeEnv::new().with_module_id(&target_module_id);
                    let type_exports = self
                        .type_level_exports(&import_env, import_file_mod)
                        .unpack(diags);
                    if type_exports.iter().next().is_some() {
                        *env = env.with_type_level(alias.name.clone(), Type::Record(type_exports));
                    }
                }
            }
        }
        Ok(())
    }

    fn find_imports<'a>(
        &'a self,
        file_mod: &'a ast::FileMod,
    ) -> HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)> {
        file_mod
            .statements
            .iter()
            .filter_map(|statement| {
                if let ast::ModStmt::Import(import_stmt) = statement {
                    let alias = import_stmt
                        .as_ref()
                        .vars
                        .last()
                        .expect("import path contains at least one segment");
                    let import_path = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect::<crate::ModuleId>();
                    let destination = self.resolve_import(import_stmt);
                    return Some((alias.name.as_str(), (import_path, destination)));
                }
                None
            })
            .collect()
    }

    fn resolve_import<'a>(
        &'a self,
        import_stmt: &'a crate::Loc<ast::ImportStmt>,
    ) -> Option<&'a ast::FileMod> {
        let import_path = import_stmt
            .as_ref()
            .vars
            .iter()
            .map(|var| var.name.clone())
            .collect::<crate::ModuleId>();
        let package_name = self.package_name_for_import(&import_path)?;
        let (_, package) = self
            .program
            .packages()
            .find(|(name, _)| *name == &package_name)?;
        let module_segments = import_path.suffix_after(&package_name)?;
        if module_segments.is_empty() {
            return None;
        }
        let module_path = module_segments
            .iter()
            .cloned()
            .collect::<crate::ModuleId>()
            .to_path_buf_with_extension("scl");
        package
            .modules()
            .find_map(|(path, file_mod)| (path == &module_path).then_some(file_mod))
    }

    fn package_name_for_import(&self, import_path: &crate::ModuleId) -> Option<crate::ModuleId> {
        self.program
            .packages()
            .map(|(name, _)| name)
            .filter(|package_name| import_path.starts_with(package_name))
            .max_by_key(|package_name| package_name.len())
            .cloned()
    }
}

fn module_id_for_path(package_id: &crate::ModuleId, path: &Path) -> crate::ModuleId {
    let mut segments = package_id.as_slice().to_vec();
    if let Some(parent) = path.parent() {
        for segment in parent.components() {
            if let Component::Normal(part) = segment {
                segments.push(part.to_string_lossy().into_owned());
            }
        }
    }

    if let Some(stem) = path.file_stem() {
        segments.push(stem.to_string_lossy().into_owned());
    }

    segments.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{TypeChecker, next_type_id};
    use crate::{
        DictType, FnType, Loc, ModuleId, Position, Program, RecordType, Span, StdSourceRepo, Type,
        TypeKind,
        ast::{
            BinaryExpr, BinaryOp, DictEntry, DictExpr, Expr, Int, RecordExpr, RecordField, StrExpr,
            UnaryExpr, UnaryOp, Var,
        },
    };

    fn checker() -> TypeChecker<'static, StdSourceRepo> {
        let program = Box::new(Program::<StdSourceRepo>::new());
        let program = Box::leak(program);
        TypeChecker::new(program)
    }

    fn loc<T>(value: T, span: Span) -> Loc<T> {
        Loc::new(value, span)
    }

    #[test]
    fn assign_type_accepts_exact_match() {
        let checker = checker();
        assert!(checker.assign_type(&Type::Int, &Type::Int).is_ok());
    }

    #[test]
    fn assign_type_accepts_non_optional_rhs_for_optional_lhs() {
        let checker = checker();
        let lhs = Type::Optional(Box::new(Type::Int));
        let rhs = Type::Int;
        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_rejects_optional_rhs_for_non_optional_lhs() {
        let checker = checker();
        let lhs = Type::Int;
        let rhs = Type::Optional(Box::new(Type::Int));
        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_error_has_causal_chain() {
        let checker = checker();
        let lhs = Type::Optional(Box::new(Type::Str));
        let rhs = Type::Int;
        let error = checker
            .assign_type(&lhs, &rhs)
            .expect_err("expected mismatch");
        let text = error.to_string();

        assert!(text.contains("Int is not assignable to Str?"));
        assert!(text.contains("Int is not assignable to Str"));
        assert!(text.contains(", because "));
    }

    #[test]
    fn assign_type_record_width_subtyping() {
        let checker = checker();
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int);
        lhs_record.insert("c".into(), Type::Bool);
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int);
        rhs_record.insert("b".into(), Type::Str);
        rhs_record.insert("c".into(), Type::Bool);
        let rhs = Type::Record(rhs_record);

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_record_depth_subtyping() {
        let checker = checker();
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Optional(Box::new(Type::Int)));
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int);
        let rhs = Type::Record(rhs_record);

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_record_missing_field_rejected() {
        let checker = checker();
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int);
        lhs_record.insert("b".into(), Type::Str);
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int);
        let rhs = Type::Record(rhs_record);

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_record_missing_optional_field_accepted() {
        let checker = checker();
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int);
        lhs_record.insert("b".into(), Type::Optional(Box::new(Type::Str)));
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int);
        let rhs = Type::Record(rhs_record);

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn record_expr_missing_optional_field_accepted() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let record_expr = loc(
            Expr::Record(RecordExpr {
                fields: vec![RecordField {
                    var: loc(
                        Var {
                            name: "a".into(),
                            cursor: None,
                        },
                        span,
                    ),
                    expr: loc(Expr::Int(Int { value: 1 }), span),
                }],
            }),
            span,
        );

        let mut expected_record = RecordType::default();
        expected_record.insert("a".into(), Type::Int);
        expected_record.insert("b".into(), Type::Optional(Box::new(Type::Str)));
        let expected_ty = Type::Record(expected_record);

        let diagnosed = checker
            .check_expr(&env, &record_expr, Some(&expected_ty))
            .expect("type check should succeed");

        assert!(
            diagnosed.diags().is_empty(),
            "expected no diagnostics for missing optional field"
        );
    }

    #[test]
    fn assign_type_record_field_not_subtype_rejected() {
        let checker = checker();
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int);
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Optional(Box::new(Type::Int)));
        let rhs = Type::Record(rhs_record);

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn record_field_mismatch_is_reported_at_field_expr_span() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let record_span = Span::new(Position::new(1, 1), Position::new(1, 10));
        let field_span = Span::new(Position::new(1, 5), Position::new(1, 6));

        let record_expr = loc(
            Expr::Record(RecordExpr {
                fields: vec![RecordField {
                    var: loc(
                        Var {
                            name: "a".into(),
                            cursor: None,
                        },
                        field_span,
                    ),
                    expr: loc(Expr::Int(Int { value: 1 }), field_span),
                }],
            }),
            record_span,
        );

        let mut expected_record = RecordType::default();
        expected_record.insert("a".into(), Type::Str);
        let expected_ty = Type::Record(expected_record);

        let diagnosed = checker
            .check_expr(&env, &record_expr, Some(&expected_ty))
            .expect("type check should succeed with diags");

        let mut diags = diagnosed.diags().iter();
        let diag = diags.next().expect("expected mismatch diagnostic");
        let (_, span) = diag.locate();
        assert_eq!(span, field_span);
        assert!(diags.next().is_none(), "expected only one diagnostic");
    }

    #[test]
    fn assign_type_dict_covariant() {
        let checker = checker();
        let lhs = Type::Dict(DictType {
            key: Box::new(Type::Optional(Box::new(Type::Str))),
            value: Box::new(Type::Optional(Box::new(Type::Int))),
        });
        let rhs = Type::Dict(DictType {
            key: Box::new(Type::Str),
            value: Box::new(Type::Int),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn dict_infers_key_value_types_from_first_entry() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let dict_expr = loc(
            Expr::Dict(DictExpr {
                entries: vec![
                    DictEntry {
                        key: loc(Expr::Int(Int { value: 1 }), span),
                        value: loc(Expr::Str(StrExpr { value: "a".into() }), span),
                    },
                    DictEntry {
                        key: loc(Expr::Int(Int { value: 2 }), span),
                        value: loc(Expr::Str(StrExpr { value: "b".into() }), span),
                    },
                ],
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &dict_expr, None)
            .expect("type check should succeed");
        assert_eq!(
            diagnosed.into_inner(),
            Type::Dict(DictType {
                key: Box::new(Type::Int),
                value: Box::new(Type::Str),
            })
        );
    }

    #[test]
    fn add_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let add_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Add,
                lhs: Box::new(loc(Expr::Int(Int { value: 1 }), span)),
                rhs: Box::new(loc(Expr::Int(Int { value: 2 }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &add_expr, None)
            .expect("type check should succeed");
        assert_eq!(diagnosed.into_inner(), Type::Int);
    }

    #[test]
    fn add_strings_returns_str() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let add_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Add,
                lhs: Box::new(loc(Expr::Str(StrExpr { value: "a".into() }), span)),
                rhs: Box::new(loc(Expr::Str(StrExpr { value: "b".into() }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &add_expr, None)
            .expect("type check should succeed");
        assert_eq!(diagnosed.into_inner(), Type::Str);
    }

    #[test]
    fn add_mismatched_types_reports_diag() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let add_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Add,
                lhs: Box::new(loc(Expr::Int(Int { value: 1 }), span)),
                rhs: Box::new(loc(Expr::Str(StrExpr { value: "b".into() }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &add_expr, None)
            .expect("type check should succeed with diags");
        assert!(matches!(diagnosed.kind, TypeKind::Never));

        let mut diags = diagnosed.diags().iter();
        let diag = diags.next().expect("expected diagnostic");
        assert!(diag.to_string().contains("invalid operands for +"));
        assert!(diags.next().is_none(), "expected only one diagnostic");
    }

    #[test]
    fn subtract_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let sub_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Sub,
                lhs: Box::new(loc(Expr::Int(Int { value: 3 }), span)),
                rhs: Box::new(loc(Expr::Int(Int { value: 1 }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &sub_expr, None)
            .expect("type check should succeed");
        assert_eq!(diagnosed.into_inner(), Type::Int);
    }

    #[test]
    fn unary_minus_float_returns_float() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let unary_expr = loc(
            Expr::Unary(UnaryExpr {
                op: UnaryOp::Negate,
                expr: Box::new(loc(
                    Expr::Float(crate::Float {
                        value: ordered_float::NotNan::new(1.5).unwrap(),
                    }),
                    span,
                )),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &unary_expr, None)
            .expect("type check should succeed");
        assert_eq!(diagnosed.into_inner(), Type::Float);
    }

    #[test]
    fn multiply_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let mul_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Mul,
                lhs: Box::new(loc(Expr::Int(Int { value: 2 }), span)),
                rhs: Box::new(loc(Expr::Int(Int { value: 4 }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &mul_expr, None)
            .expect("type check should succeed");
        assert_eq!(diagnosed.into_inner(), Type::Int);
    }

    #[test]
    fn divide_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let div_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Div,
                lhs: Box::new(loc(Expr::Int(Int { value: 8 }), span)),
                rhs: Box::new(loc(Expr::Int(Int { value: 2 }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &div_expr, None)
            .expect("type check should succeed");
        assert_eq!(diagnosed.into_inner(), Type::Int);
    }

    #[test]
    fn equality_returns_bool_and_warns_on_disjoint_types() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let eq_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Eq,
                lhs: Box::new(loc(Expr::Int(Int { value: 1 }), span)),
                rhs: Box::new(loc(Expr::Str(StrExpr { value: "x".into() }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &eq_expr, None)
            .expect("type check should succeed with diags");
        assert_eq!(diagnosed.as_ref(), &Type::Bool);

        let mut diags = diagnosed.diags().iter();
        let diag = diags.next().expect("expected warning");
        assert!(matches!(diag.level(), crate::DiagLevel::Warning));
    }

    #[test]
    fn comparison_requires_numeric_operands() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let cmp_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::Lt,
                lhs: Box::new(loc(Expr::Int(Int { value: 1 }), span)),
                rhs: Box::new(loc(Expr::Str(StrExpr { value: "x".into() }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &cmp_expr, None)
            .expect("type check should succeed with diags");
        assert!(matches!(diagnosed.kind, TypeKind::Never));
    }

    #[test]
    fn logical_operators_require_bool() {
        let checker = checker();
        let module_id = ModuleId::default();
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let and_expr = loc(
            Expr::Binary(BinaryExpr {
                op: BinaryOp::And,
                lhs: Box::new(loc(Expr::Bool(crate::Bool { value: true }), span)),
                rhs: Box::new(loc(Expr::Int(Int { value: 1 }), span)),
            }),
            span,
        );

        let diagnosed = checker
            .check_expr(&env, &and_expr, None)
            .expect("type check should succeed with diags");
        assert!(matches!(diagnosed.kind, TypeKind::Never));
    }

    #[test]
    fn assign_type_fn_exact_match() {
        let checker = checker();
        let ty = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });

        assert!(checker.assign_type(&ty, &ty).is_ok());
    }

    #[test]
    fn assign_type_fn_covariant_return() {
        let checker = checker();
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Optional(Box::new(Type::Str))),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_fn_rejects_return_type_mismatch() {
        let checker = checker();
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Bool),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_fn_rejects_param_count_mismatch() {
        let checker = checker();
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int, Type::Bool],
            ret: Box::new(Type::Str),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_fn_rejects_param_type_mismatch() {
        let checker = checker();
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Bool],
            ret: Box::new(Type::Str),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_fn_rejects_non_fn_rhs() {
        let checker = checker();
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });

        assert!(checker.assign_type(&lhs, &Type::Int).is_err());
    }

    #[test]
    fn assign_type_generic_lhs_fn_concrete_rhs_rejected() {
        let checker = checker();
        let id_a = next_type_id();

        // fn<A>(A) A  assigned from  fn(Int) Int — fails: concrete fn can't serve as polymorphic fn
        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Any)],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_generic_lhs_fn_concrete_rhs_tight_bound() {
        let checker = checker();
        let id_a = next_type_id();

        // fn<A <: Int>(A) Int  assigned from  fn(Int) Int — succeeds: lhs instantiated at Int
        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Int)],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Int),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_concrete_lhs_generic_rhs() {
        let checker = checker();
        let id_a = next_type_id();

        // fn(Int) Int  assigned from  fn<A>(A) A — unification solves A=Int
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Any)],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_both_generic_fns() {
        let checker = checker();
        let id_a = next_type_id();
        let id_b = next_type_id();

        // fn<A>(A) A  assigned from  fn<B>(B) B — identical structure, succeeds
        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Any)],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_b, Type::Any)],
            params: vec![Type::Var(id_b)],
            ret: Box::new(Type::Var(id_b)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_bounded_generic_rhs_succeeds() {
        let checker = checker();
        let id_t = next_type_id();

        // fn(Int) Int  assigned from  fn<T <: Int?>(T) T — T=Int satisfies Never <: Int <: Int?
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_t, Type::Optional(Box::new(Type::Int)))],
            params: vec![Type::Var(id_t)],
            ret: Box::new(Type::Var(id_t)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_bounded_generic_rhs_fails_bound_violation() {
        let checker = checker();
        let id_t = next_type_id();

        // fn(Int?) Int?  assigned from  fn<T <: Int>(T) T — fails: Int? is not <: Int
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Optional(Box::new(Type::Int))],
            ret: Box::new(Type::Optional(Box::new(Type::Int))),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_t, Type::Int)],
            params: vec![Type::Var(id_t)],
            ret: Box::new(Type::Var(id_t)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_contravariant_generic_fn() {
        let checker = checker();
        let id_t = next_type_id();

        // fn(fn(Int) Int) Int  <:  fn<T <: Int>(fn(T) Int) T
        //
        // assign_type(lhs, rhs) checks rhs <: lhs.
        // lhs = fn(fn(Int) Int) Int (the expected type)
        // rhs = fn<T <: Int>(fn(T) Int) T (the generic type being assigned)
        //
        // Unification of rhs against lhs:
        // - Params (Contravariant): lhs_p = fn(Int) Int, rhs_p = fn(T) Int
        //   - Inner params (flip to Covariant): lhs=Int, rhs=T → T gets upper bound Int
        //   - Inner return (keep Contravariant): lhs=Int, rhs=Int → ✓
        // - Return (Covariant): lhs=Int, rhs=T → T gets upper bound Int
        //
        // T: lower=Never, upper=min(Int, Int, Int)=Int. Never <: Int ✓
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Fn(FnType {
                type_params: vec![],
                params: vec![Type::Int],
                ret: Box::new(Type::Int),
            })],
            ret: Box::new(Type::Int),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_t, Type::Int)],
            params: vec![Type::Fn(FnType {
                type_params: vec![],
                params: vec![Type::Var(id_t)],
                ret: Box::new(Type::Int),
            })],
            ret: Box::new(Type::Var(id_t)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_fn_contravariant_params() {
        let checker = checker();

        // assign_type(lhs, rhs) checks rhs <: lhs.
        // fn(Int?) Int <: fn(Int) Int — contravariant params: Int <: Int? ✓
        // So lhs = fn(Int) Int, rhs = fn(Int?) Int.
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Optional(Box::new(Type::Int))],
            ret: Box::new(Type::Int),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_fn_contravariant_params_reject() {
        let checker = checker();

        // fn(Int) Int is NOT <: fn(Int?) Int — contravariant: Int? is NOT <: Int
        // So lhs = fn(Int?) Int, rhs = fn(Int) Int.
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Optional(Box::new(Type::Int))],
            ret: Box::new(Type::Int),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_both_generic_tighter_bound_succeeds() {
        let checker = checker();
        let id_a = next_type_id();
        let id_b = next_type_id();

        // fn<A <: Int?>(A) A  <:  fn<B <: Int>(B) B
        // F-sub rule: rhs bound (Int) <: lhs bound (Int?) ✓, then body check with B having bound Int
        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Optional(Box::new(Type::Int)))],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_b, Type::Int)],
            params: vec![Type::Var(id_b)],
            ret: Box::new(Type::Var(id_b)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_ok());
    }

    #[test]
    fn assign_type_both_generic_looser_bound_fails() {
        let checker = checker();
        let id_a = next_type_id();
        let id_b = next_type_id();

        // fn<A <: Int>(A) A  is NOT <:  fn<B <: Int?>(B) B
        // F-sub: rhs bound (Int?) <: lhs bound (Int)? No, Int? is not <: Int.
        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Int)],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_b, Type::Optional(Box::new(Type::Int)))],
            params: vec![Type::Var(id_b)],
            ret: Box::new(Type::Var(id_b)),
        });

        assert!(checker.assign_type(&lhs, &rhs).is_err());
    }

    #[test]
    fn assign_type_var_to_bound_via_upper_bound() {
        use std::collections::HashMap;
        let checker = checker();
        let id = next_type_id();
        // T <: Int? means T should be assignable to Int?
        let bounds = HashMap::from([(id, Type::Optional(Box::new(Type::Int)))]);
        assert!(
            checker
                .assign_type_with_bounds(
                    &Type::Optional(Box::new(Type::Int)),
                    &Type::Var(id),
                    &bounds
                )
                .is_ok()
        );
    }

    #[test]
    fn assign_type_var_to_stricter_than_bound_fails() {
        use std::collections::HashMap;
        let checker = checker();
        let id = next_type_id();
        // T <: Int? means T is NOT necessarily assignable to Int (could be nil)
        let bounds = HashMap::from([(id, Type::Optional(Box::new(Type::Int)))]);
        assert!(
            checker
                .assign_type_with_bounds(&Type::Int, &Type::Var(id), &bounds)
                .is_err()
        );
    }

    #[test]
    fn assign_type_var_with_record_bound_allows_field_access() {
        use std::collections::HashMap;
        let checker = checker();
        let id = next_type_id();
        let mut record = RecordType::default();
        record.insert("x".to_string(), Type::Int);
        let bounds = HashMap::from([(id, Type::Record(record.clone()))]);
        // T <: {x: Int} means T should be assignable to {x: Int}
        assert!(
            checker
                .assign_type_with_bounds(&Type::Record(record), &Type::Var(id), &bounds)
                .is_ok()
        );
    }

    #[test]
    fn assign_type_var_with_fn_bound_allows_fn_assignment() {
        use std::collections::HashMap;
        let checker = checker();
        let id = next_type_id();
        let fn_ty = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Int),
        });
        let bounds = HashMap::from([(id, fn_ty.clone())]);
        // T <: fn(Int) Int means T should be assignable to fn(Int) Int
        assert!(
            checker
                .assign_type_with_bounds(&fn_ty, &Type::Var(id), &bounds)
                .is_ok()
        );
    }

    // --- Type declaration tests ---

    fn check_module(source: &str) -> crate::Diagnosed<Type> {
        let module_id = ModuleId::default();
        let file_mod = crate::parser::parse_file_mod(source, &module_id).into_inner();
        let program = Box::new(Program::<StdSourceRepo>::new());
        let program: &'static Program<StdSourceRepo> = Box::leak(program);
        let checker = TypeChecker::new(program);
        let env = super::TypeEnv::new().with_module_id(&module_id);
        checker
            .check_file_mod(&env, &file_mod)
            .expect("type check should not error")
    }

    /// Helper to extract an export's Fn type from a check_module result,
    /// unfolding any iso-recursive wrapper.
    fn get_export_fn(diagnosed: &crate::Diagnosed<Type>, name: &str) -> FnType {
        let TypeKind::Record(exports) = &diagnosed.as_ref().kind else {
            panic!("expected record type, got: {}", diagnosed.as_ref());
        };
        let Some(ty) = exports.get(name) else {
            panic!("expected export '{name}'");
        };
        let unfolded = ty.unfold();
        let TypeKind::Fn(fn_ty) = &unfolded.kind else {
            panic!("expected fn type for '{name}', got: {}", ty);
        };
        fn_ty.clone()
    }

    #[test]
    fn simple_type_alias_resolves() {
        let diagnosed = check_module("type Port Int\nexport let p = fn(x: Port) x");
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let fn_ty = get_export_fn(&diagnosed, "p");
        assert_eq!(fn_ty.params[0], Type::Int);
        assert_eq!(*fn_ty.ret, Type::Int);
    }

    #[test]
    fn generic_type_def_produces_fn_type() {
        let diagnosed = check_module(
            "type Pair<A, B> { fst: A, snd: B }\nexport let p = fn(x: Pair<Int, Str>) x.fst",
        );
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let fn_ty = get_export_fn(&diagnosed, "p");
        // param should be { fst: Int, snd: Str }
        let TypeKind::Record(param_rec) = &fn_ty.params[0].kind else {
            panic!("expected record param type, got: {}", fn_ty.params[0]);
        };
        assert_eq!(param_rec.get("fst"), Some(&Type::Int));
        assert_eq!(param_rec.get("snd"), Some(&Type::Str));
        // return should be Int
        assert_eq!(*fn_ty.ret, Type::Int);
    }

    #[test]
    fn forward_reference_between_type_defs() {
        // B references A which is defined after B
        let diagnosed = check_module("type B A\ntype A Int\nexport let f = fn(x: B) x");
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let fn_ty = get_export_fn(&diagnosed, "f");
        assert_eq!(fn_ty.params[0], Type::Int);
    }

    #[test]
    fn type_def_and_value_coexist() {
        // Same name as both a type and a value
        let diagnosed = check_module(
            "type Config { port: Int }\nexport let Config = fn(port: Int) { port: port }",
        );
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let TypeKind::Record(exports) = &diagnosed.as_ref().kind else {
            panic!("expected record type");
        };
        // The value export should exist
        assert!(exports.get("Config").is_some());
    }

    #[test]
    fn exported_type_appears_in_type_level_exports() {
        let module_id = ModuleId::default();
        let file_mod = crate::parser::parse_file_mod(
            "export type Config { host: Str, port: Int }",
            &module_id,
        )
        .into_inner();
        let program = Box::new(Program::<StdSourceRepo>::new());
        let program: &'static Program<StdSourceRepo> = Box::leak(program);
        let checker = TypeChecker::new(program);
        let env = super::TypeEnv::new().with_module_id(&module_id);
        let diagnosed = checker.type_level_exports(&env, &file_mod);
        assert!(!diagnosed.diags().has_errors());
        let type_exports = diagnosed.into_inner();
        let Some(config_ty) = type_exports.get("Config") else {
            panic!("expected exported type 'Config'");
        };
        let TypeKind::Record(config_rec) = &config_ty.kind else {
            panic!("expected record type");
        };
        assert_eq!(config_rec.get("host"), Some(&Type::Str));
        assert_eq!(config_rec.get("port"), Some(&Type::Int));
    }

    #[test]
    fn unknown_type_in_type_def_reports_error() {
        let diagnosed = check_module("type Foo Nonexistent\nexport let f = fn(x: Foo) x");
        assert!(
            diagnosed.diags().has_errors(),
            "expected error for unknown type"
        );
    }

    #[test]
    fn wrong_type_arg_count_reports_error() {
        let diagnosed =
            check_module("type Pair<A, B> { fst: A, snd: B }\nexport let f = fn(x: Pair<Int>) x");
        assert!(
            diagnosed.diags().has_errors(),
            "expected error for wrong arg count"
        );
    }

    #[test]
    fn type_application_to_non_generic_reports_error() {
        let diagnosed = check_module("type Name Str\nexport let f = fn(x: Name<Int>) x");
        assert!(
            diagnosed.diags().has_errors(),
            "expected error for applying args to non-generic"
        );
    }

    // --- Recursive globals with free variable constraints ---

    #[test]
    fn recursive_global_record_member_access_no_error() {
        // Accessing a member of a recursive global within its own body should
        // not produce "undefined member" errors.
        let diagnosed =
            check_module("let node = { value: 1, child: node }\nexport let v = node.value");
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let TypeKind::Record(exports) = &diagnosed.as_ref().kind else {
            panic!("expected record type");
        };
        // The exported `v` should be Int (possibly wrapped in IsoRec)
        let v_ty = exports.get("v").expect("expected 'v' export").unfold();
        assert_eq!(v_ty, Type::Int);
    }

    #[test]
    fn recursive_global_self_reference_produces_isorec() {
        // A recursive global that references itself should produce an IsoRec type.
        let diagnosed = check_module("export let node = { value: 1, child: node }");
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let TypeKind::Record(exports) = &diagnosed.as_ref().kind else {
            panic!("expected record type");
        };
        let node_ty = exports.get("node").expect("expected 'node' export");
        // Should be IsoRec since `node` references itself
        assert!(
            matches!(node_ty.kind, TypeKind::IsoRec(_, _)),
            "expected IsoRec type, got: {node_ty}"
        );
    }

    #[test]
    fn recursive_global_non_recursive_body_simplifies() {
        // A global that doesn't reference itself should not have a meaningful IsoRec.
        let diagnosed = check_module("export let x = 42");
        assert!(!diagnosed.diags().has_errors());
        let TypeKind::Record(exports) = &diagnosed.as_ref().kind else {
            panic!("expected record type");
        };
        let x_ty = exports.get("x").expect("expected 'x' export");
        // Should simplify to Int (IsoRec with unused var displays as just the body)
        let unfolded = x_ty.unfold();
        assert_eq!(unfolded, Type::Int);
    }

    #[test]
    fn free_var_constraints_solve_basic() {
        // Direct unit test for the constraint solving mechanism.
        let mut constraints = super::FreeVarConstraints::new();
        let primary_id = 100;
        let member_id = 101;
        constraints.register(primary_id);
        constraints.register(member_id);

        // Constrain primary to have a field "value" of type Var(member_id)
        let mut record = RecordType::default();
        record.insert("value".into(), Type::Var(member_id));
        constraints.constrain(primary_id, Type::Record(record));

        // Body type: { value: Int, child: Var(primary_id) }
        let mut body_record = RecordType::default();
        body_record.insert("value".into(), Type::Int);
        body_record.insert("child".into(), Type::Var(primary_id));
        let body_type = Type::Record(body_record);

        let solved = constraints.solve(primary_id, &body_type);

        // member_id should resolve to Int
        let member_solution = solved.iter().find(|(id, _)| *id == member_id);
        assert_eq!(
            member_solution.map(|(_, ty)| ty),
            Some(&Type::Int),
            "member_id should resolve to Int via unification"
        );
    }

    #[test]
    fn free_var_constraints_solve_fn_return() {
        // Test that calling a free var as a function produces solvable constraints.
        let mut constraints = super::FreeVarConstraints::new();
        let primary_id = 200;
        let ret_id = 201;
        constraints.register(primary_id);
        constraints.register(ret_id);

        // Constrain primary to be fn(Int) Var(ret_id)
        constraints.constrain(
            primary_id,
            Type::Fn(FnType {
                type_params: vec![],
                params: vec![Type::Int],
                ret: Box::new(Type::Var(ret_id)),
            }),
        );

        // Body type: fn(x: Int) Str (a function from Int to Str)
        let body_type = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int],
            ret: Box::new(Type::Str),
        });

        let solved = constraints.solve(primary_id, &body_type);

        // ret_id should resolve to Str
        let ret_solution = solved.iter().find(|(id, _)| *id == ret_id);
        assert_eq!(
            ret_solution.map(|(_, ty)| ty),
            Some(&Type::Str),
            "return type var should resolve to Str via unification"
        );
    }

    #[test]
    fn recursive_global_fn_if_branches_constrain_return() {
        // A recursive function whose return type is inferred from the non-recursive
        // branch should not produce type errors on the recursive call branch.
        let diagnosed = check_module(
            "let test = fn(b: Bool) if (b) 123 else test(false)\nexport let v = test(true)",
        );
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
    }
}
