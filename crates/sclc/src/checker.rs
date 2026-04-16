use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    CompletionMember, CursorIdentifier, DiagList, Diagnosed, DictType, FnType, GlobalKey,
    RawModuleId, RecordType, Type, TypeError, TypeIssue, TypeKind, ast,
};
use thiserror::Error;

// ═══════════════════════════════════════════════════════════════════════════════
// ID generation
// ═══════════════════════════════════════════════════════════════════════════════

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

pub fn next_type_id() -> usize {
    NEXT_TYPE_ID.fetch_add(1, Ordering::Relaxed)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Diagnostic error types
// ═══════════════════════════════════════════════════════════════════════════════

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
#[error("optional chaining (?.) on non-optional type {ty}")]
pub struct OptionalChainOnNonOptional {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for OptionalChainOnNonOptional {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("nil coalescing (??) on non-optional type {ty}")]
pub struct NilCoalesceOnNonOptional {
    pub module_id: crate::ModuleId,
    pub ty: Type,
    pub span: crate::Span,
}

impl crate::Diag for NilCoalesceOnNonOptional {
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
#[error("path not found: {resolved_path}")]
pub struct InvalidPath {
    pub module_id: crate::ModuleId,
    pub resolved_path: String,
    pub span: crate::Span,
}

impl crate::Diag for InvalidPath {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
#[error("type annotation required for parameter")]
pub struct MissingParameterType {
    pub module_id: crate::ModuleId,
    pub span: crate::Span,
}

impl crate::Diag for MissingParameterType {
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
#[error("cyclic dependency between {names} — recursive bindings must be functions")]
pub struct CyclicDependency {
    pub module_id: crate::ModuleId,
    pub names: String,
    pub span: crate::Span,
}

impl crate::Diag for CyclicDependency {
    fn locate(&self) -> (crate::ModuleId, crate::Span) {
        (self.module_id.clone(), self.span)
    }
}

#[derive(Error, Debug)]
pub enum TypeCheckError {
    #[error("module id missing during type checking")]
    ModuleIdMissing,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Free variable constraints
// ═══════════════════════════════════════════════════════════════════════════════

/// Accumulates lower-bound constraints for free type variables during recursive
/// global checking. Shared (via `Rc<RefCell<…>>`) across all derived environments
/// while checking a single global's body.
#[derive(Default)]
pub(crate) struct FreeVarConstraints {
    /// Maps free var ID → accumulated lower bound.
    /// Starts as `Type::Never` and is tightened upward as constraints arrive.
    lower_bounds: HashMap<usize, Type>,
}

impl FreeVarConstraints {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a new free variable with initial lower bound `Never`.
    pub(crate) fn register(&mut self, id: usize) {
        self.lower_bounds.insert(id, Type::Never());
    }

    /// Returns true if `id` is a tracked free variable.
    fn contains(&self, id: usize) -> bool {
        self.lower_bounds.contains_key(&id)
    }

    /// Tighten the lower bound for a free variable by replacing `Never` with
    /// the first concrete constraint. Subsequent constraints are merged
    /// structurally when possible (e.g. record types are merged field-by-field).
    pub(crate) fn constrain(&mut self, id: usize, new_lower: Type) {
        let entry = self
            .lower_bounds
            .get_mut(&id)
            .expect("free var must be registered");
        if matches!(entry.kind, TypeKind::Never) {
            *entry = new_lower;
        } else if !matches!(new_lower.kind, TypeKind::Never) {
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
                }
            }
        }
    }

    /// Solve constraints given that `primary_id` equals `body_type`.
    pub(crate) fn solve(&self, primary_id: usize, body_type: &Type) -> Vec<(usize, Type)> {
        self.solve_multi(&[(primary_id, body_type)])
    }

    /// Solve constraints for multiple bindings simultaneously.
    /// Each `(id, body_type)` pair says "the free variable `id` has synthesized body type `body_type`".
    /// Returns substitutions for all tracked free variables.
    pub(crate) fn solve_multi(&self, bindings: &[(usize, &Type)]) -> Vec<(usize, Type)> {
        let mut solutions: HashMap<usize, Type> = HashMap::new();
        let primary_ids: HashSet<usize> = bindings.iter().map(|(id, _)| *id).collect();

        for &(id, body_type) in bindings {
            if let Some(constraint) = self.lower_bounds.get(&id) {
                Self::unify_constraint(constraint, body_type, &mut solutions);
            }
        }

        self.lower_bounds
            .iter()
            .map(|(id, bound)| {
                if primary_ids.contains(id) {
                    (*id, Type::Never())
                } else if let Some(solved) = solutions.get(id) {
                    (*id, solved.clone())
                } else if matches!(bound.kind, TypeKind::Never) {
                    (*id, Type::Never())
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
            // Skip identity mappings (Var(x) → Var(x)) — they mask lower bounds.
            (TypeKind::Var(id), TypeKind::Var(cid)) if id == cid => {}
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

// ═══════════════════════════════════════════════════════════════════════════════
// TypeEnv
// ═══════════════════════════════════════════════════════════════════════════════

/// Heap-allocated maps that make up the mutable portion of a type environment.
/// Boxed to keep `TypeEnv` small on the stack (~48 bytes instead of ~224).
#[derive(Clone, Debug)]
pub(crate) struct TypeEnvMaps<'a> {
    locals: HashMap<&'a str, (crate::Span, Type)>,
    type_vars: HashMap<String, Type>,
    /// Upper bounds for type variable IDs (used during function body checking).
    pub(crate) type_var_bounds: HashMap<usize, Type>,
    /// Type-level bindings from `type` declarations and imports (separate namespace from values).
    /// Each entry stores the type and an optional doc comment.
    type_level: HashMap<String, (Type, Option<String>)>,
}

type GlobalsMap<'a> = HashMap<&'a str, (crate::Span, &'a crate::Loc<ast::Expr>, Option<&'a str>)>;

// ═══════════════════════════════════════════════════════════════════════════════
// GlobalTypeEnv — accumulated type results across SCC iterations
// ═══════════════════════════════════════════════════════════════════════════════

/// Accumulated global type environment, built up as SCCs are processed in
/// topological order. `TypeEnv` borrows this to resolve globals and imports
/// without copying data into each per-SCC environment.
#[derive(Clone, Debug, Default)]
pub struct GlobalTypeEnv {
    types: HashMap<GlobalKey, Type>,
    /// Per-module import alias → target RawModuleId.
    import_maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>,
    /// Raw IDs of modules whose body is `.scle`. The `Alias.member` →
    /// `Global(alias_target, member)` shortcut only applies to `.scl`
    /// targets; SCLE modules have no globals and must go through ordinary
    /// property access on the module's value.
    scle_modules: std::collections::HashSet<RawModuleId>,
}

impl GlobalTypeEnv {
    pub fn new(import_maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>) -> Self {
        Self {
            types: HashMap::new(),
            import_maps,
            scle_modules: std::collections::HashSet::new(),
        }
    }

    pub fn insert(&mut self, key: GlobalKey, ty: Type) {
        self.types.insert(key, ty);
    }

    pub fn import_maps(&self) -> &HashMap<RawModuleId, HashMap<String, RawModuleId>> {
        &self.import_maps
    }

    /// Mark a module id as SCLE.
    pub fn mark_scle_module(&mut self, raw_id: RawModuleId) {
        self.scle_modules.insert(raw_id);
    }

    /// Whether the module at `raw_id` is an SCLE module.
    pub fn is_scle_module(&self, raw_id: &[String]) -> bool {
        self.scle_modules.contains(raw_id)
    }

    /// Merge additional import maps into this environment.
    pub fn merge_import_maps(&mut self, maps: HashMap<RawModuleId, HashMap<String, RawModuleId>>) {
        for (raw_id, aliases) in maps {
            self.import_maps.entry(raw_id).or_default().extend(aliases);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&GlobalKey, &Type)> {
        self.types.iter()
    }

    pub fn get(&self, key: &GlobalKey) -> Option<&Type> {
        self.types.get(key)
    }

    /// Resolve an import alias to its target RawModuleId.
    pub fn resolve_import_alias(
        &self,
        alias: &str,
        raw_module_id: &[String],
    ) -> Option<&RawModuleId> {
        self.import_maps
            .get(raw_module_id)
            .and_then(|imports| imports.get(alias))
    }

    /// Resolve a value-level variable name in the context of a module.
    /// Checks same-module globals first, then import aliases.
    pub fn resolve_variable(&self, name: &str, raw_module_id: &[String]) -> Option<&Type> {
        // Same-module global?
        let global_key = GlobalKey::Global(raw_module_id.to_vec(), name.to_string());
        if let Some(ty) = self.types.get(&global_key) {
            return Some(ty);
        }
        // Import alias?
        if let Some(imports) = self.import_maps.get(raw_module_id)
            && let Some(target_raw_id) = imports.get(name)
        {
            let module_key = GlobalKey::ModuleValue(target_raw_id.clone());
            return self.types.get(&module_key);
        }
        None
    }

    /// Resolve a type-level name in the context of a module.
    /// Checks same-module type declarations first, then import aliases.
    pub fn resolve_type(&self, name: &str, raw_module_id: &[String]) -> Option<&Type> {
        // Same-module type decl?
        let td_key = GlobalKey::TypeDecl(raw_module_id.to_vec(), name.to_string());
        if let Some(ty) = self.types.get(&td_key) {
            return Some(ty);
        }
        // Import alias (type-level)?
        if let Some(imports) = self.import_maps.get(raw_module_id)
            && let Some(target_raw_id) = imports.get(name)
        {
            let module_key = GlobalKey::ModuleTypeLevel(target_raw_id.clone());
            return self.types.get(&module_key);
        }
        None
    }
}

pub struct TypeEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    raw_module_id: Option<&'a RawModuleId>,
    pub(crate) global_env: &'a GlobalTypeEnv,
    globals: Option<&'a GlobalsMap<'a>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>>,
    pub(crate) maps: Box<TypeEnvMaps<'a>>,
    /// Cursor for reference tracking. Shared (via Arc) across all derived envs.
    pub(crate) cursor: Option<crate::Cursor>,
    /// Free variable constraints for recursive global checking.
    pub(crate) free_vars: Option<Rc<RefCell<FreeVarConstraints>>>,
}

impl<'a> TypeEnv<'a> {
    pub fn new(global_env: &'a GlobalTypeEnv) -> Self {
        Self {
            module_id: None,
            raw_module_id: None,
            global_env,
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
            raw_module_id: self.raw_module_id,
            global_env: self.global_env,
            globals: self.globals,
            imports: self.imports,
            maps: self.maps.clone(),
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn with_globals(&self, globals: &'a GlobalsMap<'a>) -> Self {
        let mut maps = self.maps.clone();
        maps.locals = HashMap::new();
        Self {
            module_id: self.module_id,
            raw_module_id: self.raw_module_id,
            global_env: self.global_env,
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
            raw_module_id: self.raw_module_id,
            global_env: self.global_env,
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
            raw_module_id: self.raw_module_id,
            global_env: self.global_env,
            globals: self.globals,
            imports: self.imports,
            maps: self.maps.clone(),
            cursor: self.cursor.clone(),
            free_vars: self.free_vars.clone(),
        }
    }

    pub fn with_raw_module_id(&self, raw_module_id: &'a RawModuleId) -> Self {
        Self {
            module_id: self.module_id,
            raw_module_id: Some(raw_module_id),
            global_env: self.global_env,
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

    pub fn with_type_level(&self, name: String, ty: Type, doc: Option<String>) -> Self {
        let mut env = self.inner();
        env.maps.type_level.insert(name, (ty, doc));
        env
    }

    /// Create a derived environment with a free variable for recursive global
    /// checking. The free variable is added to the shared constraint set and
    /// bound as a local so that name resolution finds it.
    pub(crate) fn with_free_var(
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

    /// Register multiple free variables in a shared constraint set.
    /// Used for mutually recursive SCC groups.
    pub(crate) fn with_free_vars(
        &self,
        vars: &[(&'a str, crate::Span, usize)],
        constraints: Rc<RefCell<FreeVarConstraints>>,
    ) -> Self {
        let mut env = self.inner();
        for &(name, span, type_id) in vars {
            constraints.borrow_mut().register(type_id);
            env.maps.locals.insert(name, (span, Type::Var(type_id)));
        }
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
    pub(crate) fn is_free_var(&self, id: usize) -> bool {
        self.free_vars
            .as_ref()
            .is_some_and(|fv| fv.borrow().contains(id))
    }

    pub fn without_locals(&self) -> Self {
        let mut maps = self.maps.clone();
        maps.locals = HashMap::new();
        Self {
            module_id: self.module_id,
            raw_module_id: self.raw_module_id,
            global_env: self.global_env,
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

    pub fn lookup_type_level(&self, name: &str) -> Option<(&Type, Option<&str>)> {
        // Check local type-level bindings first.
        if let Some(entry) = self.maps.type_level.get(name) {
            return Some((&entry.0, entry.1.as_deref()));
        }
        // Fall through to global type env.
        if let Some(raw_id) = self.raw_module_id
            && let Some(ty) = self.global_env.resolve_type(name, raw_id)
        {
            return Some((ty, None));
        }
        None
    }

    pub fn lookup_local(&self, name: &str) -> Option<&(crate::Span, Type)> {
        self.maps.locals.get(name)
    }

    pub fn lookup_global(
        &self,
        name: &str,
    ) -> Option<(crate::Span, &crate::Loc<ast::Expr>, Option<&'a str>)> {
        self.globals
            .and_then(|globals| globals.get(name))
            .map(|(span, expr, doc)| (*span, *expr, *doc))
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

    pub fn raw_module_id(&self) -> Option<&RawModuleId> {
        self.raw_module_id
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

// ═══════════════════════════════════════════════════════════════════════════════
// TypeChecker struct
// ═══════════════════════════════════════════════════════════════════════════════

pub struct TypeChecker<'p> {
    /// Module map for import resolution.
    pub(crate) modules: &'p HashMap<crate::ModuleId, ast::FileMod>,
    /// Package names for import path splitting and IDE completions.
    pub(crate) package_names: Vec<crate::PackageId>,
    /// Cache for resolved global expression types (keyed by expression pointer).
    pub(crate) global_cache: RefCell<HashMap<*const crate::Loc<ast::Expr>, Type>>,
    /// Cache for resolved import module types (keyed by FileMod pointer).
    pub(crate) import_cache: RefCell<HashMap<*const ast::FileMod, Type>>,
    /// Cache for type-level exports (keyed by FileMod pointer).
    pub(crate) type_level_cache: RefCell<HashMap<*const ast::FileMod, RecordType>>,
}

impl<'p> TypeChecker<'p> {
    /// Create a TypeChecker from a module map and package names.
    pub fn from_modules(
        modules: &'p HashMap<crate::ModuleId, ast::FileMod>,
        package_names: Vec<crate::PackageId>,
    ) -> Self {
        Self {
            modules,
            package_names,
            global_cache: RefCell::new(HashMap::new()),
            import_cache: RefCell::new(HashMap::new()),
            type_level_cache: RefCell::new(HashMap::new()),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Subsumption check
    // ═══════════════════════════════════════════════════════════════════════════

    /// Validate that `actual_ty` is assignable to `expected_ty`, emitting a
    /// diagnostic at `span` if not. For free variables, constrains instead
    /// of erroring. Returns `actual_ty` unchanged (preserves synthesis result).
    pub(crate) fn subsumption_check(
        &self,
        env: &TypeEnv<'_>,
        span: crate::Span,
        actual_ty: Type,
        expected_ty: &Type,
        diags: &mut DiagList,
    ) -> Result<Type, TypeCheckError> {
        if let TypeKind::Var(id) = &actual_ty.kind
            && env.is_free_var(*id)
        {
            if let Some(fv) = &env.free_vars {
                fv.borrow_mut().constrain(*id, expected_ty.clone());
            }
        } else if let TypeKind::Var(id) = &expected_ty.kind
            && env.is_free_var(*id)
        {
            if let Some(fv) = &env.free_vars {
                fv.borrow_mut().constrain(*id, actual_ty.clone());
            }
        } else if let Err(error) =
            crate::assign_type_with_bounds(expected_ty, &actual_ty, &env.maps.type_var_bounds)
        {
            diags.push(InvalidType {
                module_id: env.module_id()?,
                error,
                span,
            });
        }

        Ok(actual_ty)
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Module-level checking
    // ═══════════════════════════════════════════════════════════════════════════

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
        let current_package = env.module_id().map(|m| m.package.clone());
        let imports = self.find_imports(
            file_mod,
            current_package
                .as_ref()
                .unwrap_or(&crate::PackageId::default()),
        );
        let mut env = env.with_globals(&globals).with_imports(&imports);

        let mut diags = DiagList::new();

        self.build_module_type_env(&mut env, file_mod, &mut diags)?;

        // Build intra-module dependency graph and compute SCCs
        let dep_graph = crate::dep_graph::build_intra_module_value_dep_graph(&globals);
        let sccs = dep_graph.compute_sccs();

        // Index let/export bindings by name for lookup
        let binding_by_name: HashMap<&str, &ast::LetBind> = file_mod
            .statements
            .iter()
            .filter_map(|s| match s {
                ast::ModStmt::Let(lb) | ast::ModStmt::Export(lb) => {
                    Some((lb.var.name.as_str(), lb))
                }
                _ => None,
            })
            .collect();

        // Process value bindings in SCC order
        for scc in &sccs {
            if scc.len() == 1 && !dep_graph.has_self_edge(&scc[0]) {
                // Non-recursive singleton: check normally
                let let_bind = binding_by_name[scc[0].name.as_str()];
                self.check_global_let_bind(&env, let_bind)?
                    .unpack(&mut diags);
            } else if scc.len() == 1 {
                // Self-recursive singleton: validate it's a function, then check
                let let_bind = binding_by_name[scc[0].name.as_str()];
                if !matches!(let_bind.expr.as_ref().as_ref(), ast::Expr::Fn(_)) {
                    diags.push(CyclicDependency {
                        module_id: env.module_id()?,
                        names: format!("`{}`", let_bind.var.name),
                        span: let_bind.var.span(),
                    });
                    // Assign Never and cache so downstream references don't re-check
                    let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
                    self.global_cache
                        .borrow_mut()
                        .insert(cache_key, Type::Never());
                } else {
                    self.check_global_let_bind(&env, let_bind)?
                        .unpack(&mut diags);
                }
            } else {
                // Multi-binding SCC: validate all are functions
                let all_fns = scc.iter().all(|bid| {
                    let lb = binding_by_name[bid.name.as_str()];
                    matches!(lb.expr.as_ref().as_ref(), ast::Expr::Fn(_))
                });

                if !all_fns {
                    let mut sorted_names: Vec<&str> =
                        scc.iter().map(|bid| bid.name.as_str()).collect();
                    sorted_names.sort();
                    let names = sorted_names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    for bid in scc {
                        let lb = binding_by_name[bid.name.as_str()];
                        diags.push(CyclicDependency {
                            module_id: env.module_id()?,
                            names: names.clone(),
                            span: lb.var.span(),
                        });
                        let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
                        self.global_cache
                            .borrow_mut()
                            .insert(cache_key, Type::Never());
                    }
                } else {
                    // All are functions: check mutually recursive group
                    self.check_recursive_scc_group(&env, scc, &binding_by_name)?
                        .unpack(&mut diags);
                }
            }
        }

        // Process non-binding statements (imports for cursor info, bare exprs)
        for statement in &file_mod.statements {
            match statement {
                ast::ModStmt::Import(_) => {
                    self.check_stmt(&env, statement)?.unpack(&mut diags);
                }
                ast::ModStmt::Expr(expr) => {
                    self.check_expr(&env, expr, None)?.unpack(&mut diags);
                }
                _ => {} // Let/Export already handled, TypeDefs handled in build_module_type_env
            }
        }

        // Collect exports in original statement order
        let mut exports = RecordType::default();
        for statement in &file_mod.statements {
            if let ast::ModStmt::Export(let_bind) = statement {
                let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
                if let Some(cached_ty) = self.global_cache.borrow().get(&cache_key) {
                    let ty = Type::IsoRec(next_type_id(), Box::new(cached_ty.clone()));
                    exports.insert_with_doc(
                        let_bind.var.name.clone(),
                        ty,
                        let_bind.doc_comment.clone(),
                    );
                }
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
        let current_package = env.module_id().map(|m| m.package.clone());
        let imports = self.find_imports(
            file_mod,
            current_package
                .as_ref()
                .unwrap_or(&crate::PackageId::default()),
        );
        let mut inner_env = env.with_globals(&globals).with_imports(&imports);

        if let Err(_err) = self.build_module_type_env(&mut inner_env, file_mod, &mut diags) {
            return Diagnosed::new(type_exports, diags);
        }

        for statement in &file_mod.statements {
            if let ast::ModStmt::ExportTypeDef(type_def) = statement
                && let Some((ty, _)) = inner_env.lookup_type_level(type_def.var.name.as_str())
            {
                type_exports.insert_with_doc(
                    type_def.var.name.clone(),
                    ty.clone(),
                    type_def.doc_comment.clone(),
                );
            }
        }

        self.type_level_cache
            .borrow_mut()
            .insert(cache_key, type_exports.clone());
        Diagnosed::new(type_exports, diags)
    }

    /// Build the type-level environment for a module:
    /// 1. Populate import type-level bindings.
    /// 2. Resolve local type defs in SCC-based topological order.
    pub(crate) fn build_module_type_env(
        &self,
        env: &mut TypeEnv<'_>,
        file_mod: &ast::FileMod,
        diags: &mut DiagList,
    ) -> Result<(), TypeCheckError> {
        self.populate_import_type_level(env, file_mod, diags)?;

        let type_defs = file_mod.find_type_defs();
        if type_defs.is_empty() {
            return Ok(());
        }

        // Build dependency graph over type declarations and compute SCCs
        let dep_graph = crate::dep_graph::build_type_dep_graph(&type_defs);
        let sccs = dep_graph.compute_sccs();

        // Index type defs by name for lookup
        let type_def_by_name: std::collections::HashMap<&str, &ast::TypeDef> = type_defs
            .iter()
            .map(|td| (td.var.name.as_str(), *td))
            .collect();

        // Process each SCC in topological order
        for scc in &sccs {
            if scc.len() == 1 && !dep_graph.has_self_edge(&scc[0]) {
                // Non-recursive singleton: resolve directly
                let td = type_def_by_name[scc[0].name.as_str()];
                let resolved_ty = self.resolve_type_def(env, td).unpack(diags);
                *env =
                    env.with_type_level(td.var.name.clone(), resolved_ty, td.doc_comment.clone());
            } else {
                // Recursive group: allocate a type variable for each member,
                // bootstrap with Var(type_id), resolve once, then wrap with
                // IsoRec where the variable actually appears in the body.
                let scc_vars: Vec<(&str, usize)> = scc
                    .iter()
                    .map(|bid| {
                        let td = type_def_by_name[bid.name.as_str()];
                        let type_id = next_type_id();
                        (td.var.name.as_str(), type_id)
                    })
                    .collect();

                // Bootstrap: register each type name as its type variable
                for &(name, type_id) in &scc_vars {
                    *env = env.with_type_level(name.to_owned(), Type::Var(type_id), None);
                }

                // Resolve each type body once (references to SCC members
                // will appear as Var(type_id) in the resolved type).
                let mut resolved: Vec<(&str, usize, Type, Option<String>)> =
                    Vec::with_capacity(scc_vars.len());
                for &(name, type_id) in &scc_vars {
                    let td = type_def_by_name[name];
                    let body = self.resolve_type_def(env, td).unpack(diags);
                    resolved.push((name, type_id, body, td.doc_comment.clone()));
                }

                // Wrap with IsoRec where the body actually references the variable
                for (name, type_id, body, doc) in resolved {
                    let ty = if body.contains_var(type_id) {
                        Type::IsoRec(type_id, Box::new(body))
                    } else {
                        body
                    };
                    *env = env.with_type_level(name.to_owned(), ty, doc);
                }
            }
        }

        // Set cursor info for all type defs
        for type_def in &type_defs {
            if let Some((cursor, _)) = &type_def.var.cursor {
                if let Some((ty, _)) = env.lookup_type_level(type_def.var.name.as_str()) {
                    cursor.set_type(ty.clone().strip_name());
                }
                cursor.set_identifier(CursorIdentifier::Type(type_def.var.name.clone()));
                if let Some(doc) = &type_def.doc_comment {
                    cursor.set_description(doc.clone());
                }
            }
        }

        Ok(())
    }

    #[inline(never)]
    #[allow(clippy::type_complexity)]
    pub fn check_stmt(
        &self,
        env: &TypeEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Diagnosed<Option<(String, Type, Option<String>)>>, TypeCheckError> {
        match stmt {
            ast::ModStmt::Import(import_stmt) => {
                let vars = &import_stmt.as_ref().vars;
                let alias = vars
                    .last()
                    .expect("import path contains at least one segment");

                // Set type on alias cursor when the import resolves
                if let Some((cursor, _)) = &alias.cursor
                    && let Some((target_module_id, Some(import_file_mod))) =
                        env.lookup_import(alias.name.as_str())
                {
                    let cache_key = import_file_mod as *const ast::FileMod;
                    let imported_ty =
                        if let Some(cached) = self.import_cache.borrow().get(&cache_key) {
                            Some(cached.clone())
                        } else {
                            let import_env =
                                TypeEnv::new(env.global_env).with_module_id(&target_module_id);
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

                // Add completion candidates for import path segments
                if let Ok(module_id) = env.module_id() {
                    self.add_import_completions(vars, &module_id.package);
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
                Ok(Diagnosed::new(
                    Some((let_bind.var.name.clone(), ty, let_bind.doc_comment.clone())),
                    diags,
                ))
            }
            ast::ModStmt::TypeDef(_) | ast::ModStmt::ExportTypeDef(_) => {
                Ok(Diagnosed::new(None, DiagList::new()))
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Type resolution
    // ═══════════════════════════════════════════════════════════════════════════

    pub(crate) fn resolve_type_expr(
        &self,
        env: &TypeEnv<'_>,
        type_expr: &crate::Loc<ast::TypeExpr>,
    ) -> Diagnosed<Type> {
        let mut diags = DiagList::new();
        let ty = match type_expr.as_ref() {
            ast::TypeExpr::Var(var) if var.name == "Any" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Any());
                }
                Type::Any()
            }
            ast::TypeExpr::Var(var) if var.name == "Int" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Int());
                }
                Type::Int()
            }
            ast::TypeExpr::Var(var) if var.name == "Float" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Float());
                }
                Type::Float()
            }
            ast::TypeExpr::Var(var) if var.name == "Bool" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Bool());
                }
                Type::Bool()
            }
            ast::TypeExpr::Var(var) if var.name == "Str" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Str());
                }
                Type::Str()
            }
            ast::TypeExpr::Var(var) if var.name == "Path" => {
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(Type::Path());
                }
                Type::Path()
            }
            ast::TypeExpr::Var(var) => {
                let (resolved, doc_comment) = if let Some(ty) =
                    env.lookup_type_var(var.name.as_str())
                {
                    (ty.clone(), None)
                } else if let Some((ty, doc)) = env.lookup_type_level(var.name.as_str()) {
                    let ty = ty.clone();
                    let ty = if !matches!(ty.kind, TypeKind::Fn(ref f) if !f.type_params.is_empty())
                    {
                        ty.with_name(var.name.clone())
                    } else {
                        ty
                    };
                    (ty, doc)
                } else {
                    if let Ok(module_id) = env.module_id() {
                        diags.push(UnknownType {
                            module_id,
                            name: var.name.clone(),
                            span: type_expr.span(),
                        });
                    }
                    (Type::Never(), None)
                };
                if let Some((cursor, _)) = &var.cursor {
                    cursor.set_type(resolved.clone().strip_name());
                    cursor.set_identifier(CursorIdentifier::Type(var.name.clone()));
                    if let Some(doc) = doc_comment {
                        cursor.set_description(doc.to_owned());
                    }
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
                        Type::Any()
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
                    let origin = env.raw_module_id().map(|m| (m.clone(), field.var.span()));
                    if let Some((cursor, _)) = &field.var.cursor {
                        cursor.set_type(field_ty.clone());
                        cursor.set_identifier(CursorIdentifier::Let(field.var.name.clone()));
                        if let Some(doc) = &field.doc_comment {
                            cursor.set_description(doc.clone());
                        }
                        if let Some((module, span)) = &origin {
                            cursor.set_declaration(module.clone(), *span);
                        }
                    }
                    resolved.insert_with_meta(
                        field.var.name.clone(),
                        field_ty,
                        field.doc_comment.clone(),
                        origin,
                    );
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
                            Type::Never()
                        } else {
                            let resolved_args: Vec<Type> = app
                                .args
                                .iter()
                                .map(|arg| self.resolve_type_expr(env, arg).unpack(&mut diags))
                                .collect();
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
                                if instantiated_bound.is_assignable_from(arg_ty).is_err()
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
                        Type::Never()
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
                        for (name, field_ty) in record_ty.iter() {
                            if name.starts_with(prefix) {
                                cursor.add_completion_candidate(
                                    crate::CompletionCandidate::Member(CompletionMember {
                                        name: name.clone(),
                                        description: record_ty.get_doc(name).map(str::to_owned),
                                        ty: Some(field_ty.clone()),
                                    }),
                                );
                            }
                        }
                    }
                }
                if let TypeKind::Never = &lhs_ty.kind {
                    Type::Never()
                } else if let TypeKind::Record(record_ty) = &lhs_ty.kind
                    && let Some(member_ty) = record_ty.get(prop_access.property.name.as_str())
                {
                    if let Some((cursor, _)) = &prop_access.property.cursor {
                        cursor.set_type(member_ty.clone());
                        cursor.set_identifier(CursorIdentifier::Type(
                            prop_access.property.name.clone(),
                        ));
                        if let Some(doc) = record_ty.get_doc(prop_access.property.name.as_str()) {
                            cursor.set_description(doc.to_owned());
                        }
                    }
                    if let Some(lhs_name) = &lhs_ty.name() {
                        member_ty
                            .clone()
                            .with_name(format!("{}.{}", lhs_name, prop_access.property.name))
                    } else {
                        member_ty.clone()
                    }
                } else {
                    if let Ok(module_id) = env.module_id() {
                        diags.push(UndefinedMember {
                            module_id,
                            name: prop_access.property.name.clone(),
                            ty: lhs_ty,
                            property: prop_access.property.clone(),
                        });
                    }
                    Type::Never()
                }
            }
        };
        Diagnosed::new(ty, diags)
    }

    /// Resolve a type definition to its underlying `Type`.
    pub fn resolve_type_def(&self, env: &TypeEnv<'_>, type_def: &ast::TypeDef) -> Diagnosed<Type> {
        let mut diags = DiagList::new();

        if type_def.type_params.is_empty() {
            let ty = self.resolve_type_expr(env, &type_def.ty).unpack(&mut diags);
            return Diagnosed::new(ty, diags);
        }

        let mut fn_env = env.inner();
        let mut type_param_entries = Vec::with_capacity(type_def.type_params.len());
        for type_param in &type_def.type_params {
            let type_id = next_type_id();
            fn_env = fn_env.with_type_var(type_param.var.name.clone(), Type::Var(type_id));
            let upper_bound = if let Some(bound_expr) = &type_param.bound {
                self.resolve_type_expr(&fn_env, bound_expr)
                    .unpack(&mut diags)
            } else {
                Type::Any()
            };
            fn_env = fn_env.with_type_var_bound(type_id, upper_bound.clone());
            type_param_entries.push((type_id, upper_bound));
        }

        let body_ty = self
            .resolve_type_expr(&fn_env, &type_def.ty)
            .unpack(&mut diags);

        Diagnosed::new(
            Type::Fn(FnType {
                type_params: type_param_entries,
                params: vec![],
                ret: Box::new(body_ty),
            }),
            diags,
        )
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Bidirectional expression checking
    // ═══════════════════════════════════════════════════════════════════════════

    /// Public API: dispatches to synthesis or checking mode.
    pub fn check_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match expected_type {
            None => self.synth_expr(env, expr),
            Some(expected) => self.check_expr_against(env, expr, expected),
        }
    }

    /// Synthesis mode: bottom-up type inference with no expected type.
    #[inline(never)]
    pub(crate) fn synth_expr(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        expr.as_ref().type_synth(self, env, expr)
    }

    /// Check mode: validate expression against an expected type, pushing errors
    /// to the most specific AST node where possible.
    #[inline(never)]
    pub(crate) fn check_expr_against(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        expected: &Type,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        expr.as_ref().type_check(self, env, expr, expected)
    }

    /// Fall-through: synthesize and then check subsumption.
    #[inline(never)]
    pub(crate) fn synth_then_subsume(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        expected: &Type,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let actual_ty = self.synth_expr(env, expr)?.unpack(&mut diags);
        let ty = self.subsumption_check(env, expr.span(), actual_ty, expected, &mut diags)?;
        Ok(Diagnosed::new(ty, diags))
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Shared helpers called by per-node AST files
    // ═══════════════════════════════════════════════════════════════════════════

    /// Handle type argument instantiation for generic functions at call sites.
    pub(crate) fn instantiate_call_type_args(
        &self,
        env: &TypeEnv<'_>,
        expr: &crate::Loc<ast::Expr>,
        call_expr: &ast::CallExpr,
        fn_ty: FnType,
        diags: &mut DiagList,
    ) -> Result<FnType, TypeCheckError> {
        if !call_expr.type_args.is_empty() {
            if fn_ty.type_params.is_empty() {
                diags.push(UnexpectedTypeArgs {
                    module_id: env.module_id()?,
                    span: expr.span(),
                });
                Ok(fn_ty)
            } else if call_expr.type_args.len() != fn_ty.type_params.len() {
                diags.push(WrongTypeArgCount {
                    module_id: env.module_id()?,
                    expected: fn_ty.type_params.len(),
                    got: call_expr.type_args.len(),
                    span: expr.span(),
                });
                let param_replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .map(|(id, _)| (*id, Type::Any()))
                    .collect();
                let ret_replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .map(|(id, _)| (*id, Type::Never()))
                    .collect();
                Ok(FnType {
                    type_params: vec![],
                    params: fn_ty
                        .params
                        .iter()
                        .map(|p| p.substitute(&param_replacements))
                        .collect(),
                    ret: Box::new(fn_ty.ret.substitute(&ret_replacements)),
                })
            } else {
                let replacements: Vec<(usize, Type)> = fn_ty
                    .type_params
                    .iter()
                    .zip(call_expr.type_args.iter())
                    .map(|((id, bound), type_arg)| {
                        let resolved = self.resolve_type_expr(env, type_arg).unpack(diags);
                        if crate::assign_type_with_bounds(
                            bound,
                            &resolved,
                            &env.maps.type_var_bounds,
                        )
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
                Ok(FnType {
                    type_params: vec![],
                    params: fn_ty
                        .params
                        .iter()
                        .map(|p| p.substitute(&replacements))
                        .collect(),
                    ret: Box::new(fn_ty.ret.substitute(&replacements)),
                })
            }
        } else if !fn_ty.type_params.is_empty() {
            // Type params remain — caller will infer type arguments from arg types.
            Ok(fn_ty)
        } else {
            Ok(fn_ty)
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Binary/unary operator helpers
    // ═══════════════════════════════════════════════════════════════════════════

    /// Compute the result type for an arithmetic binary operator (+, -, *, /).
    /// Returns `None` if the operand types are invalid for the operator.
    pub(crate) fn arithmetic_result(
        op: ast::BinaryOp,
        lhs: &TypeKind,
        rhs: &TypeKind,
    ) -> Option<Type> {
        match (lhs, rhs) {
            (TypeKind::Int, TypeKind::Int) => Some(Type::Int()),
            (TypeKind::Float, TypeKind::Float)
            | (TypeKind::Int, TypeKind::Float)
            | (TypeKind::Float, TypeKind::Int) => Some(Type::Float()),
            (TypeKind::Str, TypeKind::Str) if matches!(op, ast::BinaryOp::Add) => Some(Type::Str()),
            _ => None,
        }
    }

    /// Compute the result type for a comparison binary operator (<, <=, >, >=).
    pub(crate) fn comparison_result(lhs: &TypeKind, rhs: &TypeKind) -> Option<Type> {
        match (lhs, rhs) {
            (TypeKind::Int, TypeKind::Int)
            | (TypeKind::Float, TypeKind::Float)
            | (TypeKind::Int, TypeKind::Float)
            | (TypeKind::Float, TypeKind::Int) => Some(Type::Bool()),
            _ => None,
        }
    }

    /// Compute the result type for a logical binary operator (&&, ||).
    pub(crate) fn logical_result(lhs: &TypeKind, rhs: &TypeKind) -> Option<Type> {
        match (lhs, rhs) {
            (TypeKind::Bool, TypeKind::Bool) => Some(Type::Bool()),
            _ => None,
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
        let solved = constraints.borrow().solve(type_id, &resolved_ty);
        let resolved_ty = resolved_ty.substitute(&solved);
        let binding_ty = annotation_ty.unwrap_or(resolved_ty);
        let cache_key = let_bind.expr.as_ref() as *const crate::Loc<ast::Expr>;
        self.global_cache
            .borrow_mut()
            .insert(cache_key, binding_ty.clone());
        let ty = Type::IsoRec(type_id, Box::new(binding_ty));
        if let Some((cursor, _)) = &let_bind.var.cursor {
            cursor.set_type(ty.clone());
            cursor.set_identifier(CursorIdentifier::Let(let_bind.var.name.clone()));
            if let Some(doc) = &let_bind.doc_comment {
                cursor.set_description(doc.clone());
            }
        }
        Ok(Diagnosed::new(ty, diags))
    }

    /// Check a mutually recursive SCC group where all bindings are function literals.
    /// Creates type variables for all bindings, checks all bodies with all variables
    /// in scope, then solves the combined constraint system.
    pub(crate) fn check_recursive_scc_group(
        &self,
        env: &TypeEnv<'_>,
        scc: &[crate::dep_graph::BindingId],
        binding_by_name: &HashMap<&str, &ast::LetBind>,
    ) -> Result<Diagnosed<()>, TypeCheckError> {
        let mut diags = DiagList::new();
        let constraints = Rc::new(RefCell::new(FreeVarConstraints::new()));

        // Create type variables for all bindings in the SCC
        let free_var_entries: Vec<(&str, crate::Span, usize)> = scc
            .iter()
            .map(|bid| {
                let lb = binding_by_name[bid.name.as_str()];
                let type_id = next_type_id();
                (lb.var.name.as_str(), lb.var.span(), type_id)
            })
            .collect();

        // Set up environment with all free vars from this SCC
        let scc_env = env
            .without_locals()
            .with_free_vars(&free_var_entries, constraints.clone());

        // Check all bodies and collect results
        let mut body_results: Vec<(usize, Type, &ast::LetBind)> = Vec::new();
        for (i, bid) in scc.iter().enumerate() {
            let lb = binding_by_name[bid.name.as_str()];
            let (_, _, type_id) = free_var_entries[i];

            let annotation_ty = lb
                .ty
                .as_ref()
                .map(|te| self.resolve_type_expr(&scc_env, te).unpack(&mut diags));

            let resolved_ty = self
                .check_expr(&scc_env, lb.expr.as_ref(), annotation_ty.as_ref())?
                .unpack(&mut diags);

            let binding_ty = annotation_ty.unwrap_or(resolved_ty);
            body_results.push((type_id, binding_ty, lb));
        }

        // Solve constraints for all bindings simultaneously
        let solve_input: Vec<(usize, &Type)> =
            body_results.iter().map(|(id, ty, _)| (*id, ty)).collect();
        let solved = constraints.borrow().solve_multi(&solve_input);

        // Apply solutions and cache results
        for (type_id, body_ty, lb) in &body_results {
            let resolved_ty = body_ty.substitute(&solved);
            let cache_key = lb.expr.as_ref() as *const crate::Loc<ast::Expr>;
            self.global_cache
                .borrow_mut()
                .insert(cache_key, resolved_ty.clone());
            let ty = Type::IsoRec(*type_id, Box::new(resolved_ty));
            if let Some((cursor, _)) = &lb.var.cursor {
                cursor.set_type(ty.clone());
                cursor.set_identifier(CursorIdentifier::Let(lb.var.name.clone()));
                if let Some(doc) = &lb.doc_comment {
                    cursor.set_description(doc.clone());
                }
            }
        }

        // Fix up cursor types set during body checking that contain unsolved vars
        if let Some(cursor) = &env.cursor {
            cursor.substitute_type(&solved);
        }

        Ok(Diagnosed::new((), diags))
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // List item checking
    // ═══════════════════════════════════════════════════════════════════════════

    pub(crate) fn check_list_item(
        &self,
        env: &TypeEnv<'_>,
        item: &ast::ListItem,
        expected_type: Option<&Type>,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        match item {
            ast::ListItem::Expr(expr) => self.check_expr(env, expr, expected_type),
            ast::ListItem::If(if_item) => {
                let mut diags = DiagList::new();
                self.check_expr(env, if_item.condition.as_ref(), Some(&Type::Bool()))?
                    .unpack(&mut diags);
                let item_ty = self
                    .check_list_item(env, if_item.then_item.as_ref(), expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(item_ty, diags))
            }
            ast::ListItem::For(for_item) => {
                let mut diags = DiagList::new();
                let iterable_ty = self
                    .synth_expr(env, for_item.iterable.as_ref())?
                    .unpack(&mut diags)
                    .unfold();
                let element_ty = match iterable_ty.kind {
                    TypeKind::List(element_ty) => *element_ty,
                    _ => {
                        diags.push(InvalidType {
                            module_id: env.module_id()?,
                            error: TypeError::new(TypeIssue::Mismatch(
                                Type::List(Box::new(Type::Any())),
                                iterable_ty,
                            )),
                            span: for_item.iterable.span(),
                        });
                        Type::Never()
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

    // ═══════════════════════════════════════════════════════════════════════════
    // Import resolution
    // ═══════════════════════════════════════════════════════════════════════════

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

                let current_package = env
                    .module_id()
                    .map(|m| m.package.clone())
                    .unwrap_or_default();
                if let Some(import_file_mod) = self.resolve_import(import_stmt, &current_package) {
                    let raw_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect();
                    let raw_segments =
                        self.resolve_self_import_segments(raw_segments, &current_package);
                    let target_module_id = self
                        .split_import_segments(&raw_segments)
                        .unwrap_or_else(|| {
                            crate::ModuleId::new(crate::PackageId::default(), raw_segments.clone())
                        });
                    let import_env = TypeEnv::new(env.global_env)
                        .with_module_id(&target_module_id)
                        .with_raw_module_id(&raw_segments);
                    let type_exports = self
                        .type_level_exports(&import_env, import_file_mod)
                        .unpack(diags);
                    if type_exports.iter().next().is_some() {
                        *env = env.with_type_level(
                            alias.name.clone(),
                            Type::Record(type_exports),
                            None,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// If `segments` starts with `Self`, replace that prefix with the
    /// current package's ID segments.
    fn resolve_self_import_segments(
        &self,
        segments: Vec<String>,
        current_package: &crate::PackageId,
    ) -> Vec<String> {
        if segments.first().map(String::as_str) == Some("Self") {
            let mut result: Vec<String> = current_package.as_slice().to_vec();
            result.extend(segments[1..].iter().cloned());
            return result;
        }
        segments
    }

    /// Split raw import segments into a `ModuleId` using known packages.
    fn split_import_segments(&self, segments: &[String]) -> Option<crate::ModuleId> {
        let package = self.package_name_for_import(segments)?;
        let pkg_len = package.len();
        let path = segments[pkg_len..].to_vec();
        Some(crate::ModuleId::new(package, path))
    }

    pub(crate) fn find_imports<'a>(
        &'a self,
        file_mod: &'a ast::FileMod,
        current_package: &crate::PackageId,
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
                    let raw_segments: Vec<String> = import_stmt
                        .as_ref()
                        .vars
                        .iter()
                        .map(|var| var.name.clone())
                        .collect();
                    let raw_segments =
                        self.resolve_self_import_segments(raw_segments, current_package);
                    let import_path =
                        self.split_import_segments(&raw_segments)
                            .unwrap_or_else(|| {
                                crate::ModuleId::new(
                                    crate::PackageId::default(),
                                    raw_segments.clone(),
                                )
                            });
                    let destination = self.resolve_import(import_stmt, current_package);
                    return Some((alias.name.as_str(), (import_path, destination)));
                }
                None
            })
            .collect()
    }

    fn resolve_import<'a>(
        &'a self,
        import_stmt: &'a crate::Loc<ast::ImportStmt>,
        current_package: &crate::PackageId,
    ) -> Option<&'a ast::FileMod> {
        let raw_segments: Vec<String> = import_stmt
            .as_ref()
            .vars
            .iter()
            .map(|var| var.name.clone())
            .collect();
        let raw_segments = self.resolve_self_import_segments(raw_segments, current_package);
        let module_id = self.split_import_segments(&raw_segments)?;
        if module_id.path.is_empty() {
            return None;
        }
        self.modules.get(&module_id)
    }

    fn package_name_for_import(&self, segments: &[String]) -> Option<crate::PackageId> {
        self.package_names
            .iter()
            .filter(|package_name| segments.starts_with(package_name.as_slice()))
            .max_by_key(|package_name| package_name.len())
            .cloned()
    }

    /// Add Module/ModuleDir completion candidates for an import path.
    fn add_import_completions(
        &self,
        vars: &[crate::Loc<ast::Var>],
        current_package: &crate::PackageId,
    ) {
        for (i, var) in vars.iter().enumerate() {
            let Some((cursor, offset)) = &var.cursor else {
                continue;
            };
            let prefix = &var.name[..*offset];

            if i == 0 {
                // First segment: suggest package names and "Self"
                for package_name in &self.package_names {
                    if let Some(first) = package_name.as_slice().first()
                        && first.starts_with(prefix)
                    {
                        cursor.add_completion_candidate(crate::CompletionCandidate::ModuleDir(
                            first.clone(),
                        ));
                    }
                }
                if "Self".starts_with(prefix) {
                    cursor.add_completion_candidate(crate::CompletionCandidate::ModuleDir(
                        "Self".to_owned(),
                    ));
                }
            } else {
                // Subsequent segments: resolve the package from prior segments,
                // build the directory path, and suggest children.
                let prior_segments: Vec<String> =
                    vars[..i].iter().map(|v| v.name.clone()).collect();
                let prior_segments =
                    self.resolve_self_import_segments(prior_segments, current_package);

                let Some(package_name) = self.package_name_for_import(&prior_segments) else {
                    continue;
                };
                // The directory path within the package
                let pkg_len = package_name.len();
                let dir_segments = &prior_segments[pkg_len..];
                let mut dir_path = PathBuf::new();
                for seg in dir_segments {
                    dir_path.push(seg);
                }

                // Children-based completion is handled by the IDE layer;
                // the TypeChecker no longer carries a children cache.
                let _ = (package_name, dir_path);
            }
        }
    }

    /// Add file/directory completion candidates for path expressions.
    ///
    /// Currently a no-op: path completions are handled by the IDE layer.
    /// The TypeChecker no longer carries a children cache.
    pub(crate) fn add_path_completions(&self, _env: &TypeEnv<'_>, _path_expr: &ast::PathExpr) {}
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{TypeChecker, next_type_id};
    use crate::{
        DictType, FnType, Loc, ModuleId, Position, RecordType, Span, Type, TypeKind,
        ast::{
            BinaryExpr, BinaryOp, DictEntry, DictExpr, Expr, Int, RecordExpr, RecordField, StrExpr,
            UnaryExpr, UnaryOp, Var,
        },
    };

    fn checker() -> TypeChecker<'static> {
        let modules = Box::leak(Box::new(HashMap::new()));
        TypeChecker::from_modules(modules, Vec::new())
    }

    fn loc<T>(value: T, span: Span) -> Loc<T> {
        Loc::new(value, span)
    }

    #[test]
    fn assign_type_accepts_exact_match() {
        assert!(Type::Int().is_assignable_from(&Type::Int()).is_ok());
    }

    #[test]
    fn assign_type_accepts_non_optional_rhs_for_optional_lhs() {
        let lhs = Type::Optional(Box::new(Type::Int()));
        let rhs = Type::Int();
        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_rejects_optional_rhs_for_non_optional_lhs() {
        let lhs = Type::Int();
        let rhs = Type::Optional(Box::new(Type::Int()));
        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_error_has_causal_chain() {
        let lhs = Type::Optional(Box::new(Type::Str()));
        let rhs = Type::Int();
        let error = lhs.is_assignable_from(&rhs).expect_err("expected mismatch");
        let text = error.to_string();

        assert!(text.contains("Int is not assignable to Str?"));
        assert!(text.contains("Int is not assignable to Str"));
        assert!(text.contains(", because "));
    }

    #[test]
    fn assign_type_record_width_subtyping() {
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int());
        lhs_record.insert("c".into(), Type::Bool());
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int());
        rhs_record.insert("b".into(), Type::Str());
        rhs_record.insert("c".into(), Type::Bool());
        let rhs = Type::Record(rhs_record);

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_record_depth_subtyping() {
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Optional(Box::new(Type::Int())));
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int());
        let rhs = Type::Record(rhs_record);

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_record_missing_field_rejected() {
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int());
        lhs_record.insert("b".into(), Type::Str());
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int());
        let rhs = Type::Record(rhs_record);

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_record_missing_optional_field_accepted() {
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int());
        lhs_record.insert("b".into(), Type::Optional(Box::new(Type::Str())));
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Int());
        let rhs = Type::Record(rhs_record);

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn record_expr_missing_optional_field_accepted() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
        let span = Span::new(Position::new(1, 1), Position::new(1, 10));

        let record_expr = loc(
            Expr::Record(RecordExpr {
                fields: vec![RecordField {
                    doc_comment: None,
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
        expected_record.insert("a".into(), Type::Int());
        expected_record.insert("b".into(), Type::Optional(Box::new(Type::Str())));
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
        let mut lhs_record = RecordType::default();
        lhs_record.insert("a".into(), Type::Int());
        let lhs = Type::Record(lhs_record);

        let mut rhs_record = RecordType::default();
        rhs_record.insert("a".into(), Type::Optional(Box::new(Type::Int())));
        let rhs = Type::Record(rhs_record);

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn record_field_mismatch_is_reported_at_field_expr_span() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
        let record_span = Span::new(Position::new(1, 1), Position::new(1, 10));
        let field_span = Span::new(Position::new(1, 5), Position::new(1, 6));

        let record_expr = loc(
            Expr::Record(RecordExpr {
                fields: vec![RecordField {
                    doc_comment: None,
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
        expected_record.insert("a".into(), Type::Str());
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
        let lhs = Type::Dict(DictType {
            key: Box::new(Type::Optional(Box::new(Type::Str()))),
            value: Box::new(Type::Optional(Box::new(Type::Int()))),
        });
        let rhs = Type::Dict(DictType {
            key: Box::new(Type::Str()),
            value: Box::new(Type::Int()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn dict_infers_key_value_types_from_first_entry() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
                key: Box::new(Type::Int()),
                value: Box::new(Type::Str()),
            })
        );
    }

    #[test]
    fn add_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.into_inner(), Type::Int());
    }

    #[test]
    fn add_strings_returns_str() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.into_inner(), Type::Str());
    }

    #[test]
    fn add_mismatched_types_reports_diag() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.into_inner(), Type::Int());
    }

    #[test]
    fn unary_minus_float_returns_float() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.into_inner(), Type::Float());
    }

    #[test]
    fn multiply_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.into_inner(), Type::Int());
    }

    #[test]
    fn divide_ints_returns_int() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.into_inner(), Type::Int());
    }

    #[test]
    fn equality_returns_bool_and_warns_on_disjoint_types() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(diagnosed.as_ref(), &Type::Bool());

        let mut diags = diagnosed.diags().iter();
        let diag = diags.next().expect("expected warning");
        assert!(matches!(diag.level(), crate::DiagLevel::Warning));
    }

    #[test]
    fn comparison_requires_numeric_operands() {
        let checker = checker();
        let module_id = ModuleId::default();
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        let ty = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });

        assert!(ty.is_assignable_from(&ty).is_ok());
    }

    #[test]
    fn assign_type_fn_covariant_return() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Optional(Box::new(Type::Str()))),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_fn_rejects_return_type_mismatch() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Bool()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_fn_rejects_param_count_mismatch() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int(), Type::Bool()],
            ret: Box::new(Type::Str()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_fn_rejects_param_type_mismatch() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Bool()],
            ret: Box::new(Type::Str()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_fn_rejects_non_fn_rhs() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });

        assert!(lhs.is_assignable_from(&Type::Int()).is_err());
    }

    #[test]
    fn assign_type_generic_lhs_fn_concrete_rhs_rejected() {
        let id_a = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Any())],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_generic_lhs_fn_concrete_rhs_tight_bound() {
        let id_a = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Int())],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Int()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_concrete_lhs_generic_rhs() {
        let id_a = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Any())],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_both_generic_fns() {
        let id_a = next_type_id();
        let id_b = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Any())],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_b, Type::Any())],
            params: vec![Type::Var(id_b)],
            ret: Box::new(Type::Var(id_b)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_bounded_generic_rhs_succeeds() {
        let id_t = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_t, Type::Optional(Box::new(Type::Int())))],
            params: vec![Type::Var(id_t)],
            ret: Box::new(Type::Var(id_t)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_bounded_generic_rhs_fails_bound_violation() {
        let id_t = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Optional(Box::new(Type::Int()))],
            ret: Box::new(Type::Optional(Box::new(Type::Int()))),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_t, Type::Int())],
            params: vec![Type::Var(id_t)],
            ret: Box::new(Type::Var(id_t)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_contravariant_generic_fn() {
        let id_t = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Fn(FnType {
                type_params: vec![],
                params: vec![Type::Int()],
                ret: Box::new(Type::Int()),
            })],
            ret: Box::new(Type::Int()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_t, Type::Int())],
            params: vec![Type::Fn(FnType {
                type_params: vec![],
                params: vec![Type::Var(id_t)],
                ret: Box::new(Type::Int()),
            })],
            ret: Box::new(Type::Var(id_t)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_fn_contravariant_params() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Optional(Box::new(Type::Int()))],
            ret: Box::new(Type::Int()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_fn_contravariant_params_reject() {
        let lhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Optional(Box::new(Type::Int()))],
            ret: Box::new(Type::Int()),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_both_generic_tighter_bound_succeeds() {
        let id_a = next_type_id();
        let id_b = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Optional(Box::new(Type::Int())))],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_b, Type::Int())],
            params: vec![Type::Var(id_b)],
            ret: Box::new(Type::Var(id_b)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_ok());
    }

    #[test]
    fn assign_type_both_generic_looser_bound_fails() {
        let id_a = next_type_id();
        let id_b = next_type_id();

        let lhs = Type::Fn(FnType {
            type_params: vec![(id_a, Type::Int())],
            params: vec![Type::Var(id_a)],
            ret: Box::new(Type::Var(id_a)),
        });
        let rhs = Type::Fn(FnType {
            type_params: vec![(id_b, Type::Optional(Box::new(Type::Int())))],
            params: vec![Type::Var(id_b)],
            ret: Box::new(Type::Var(id_b)),
        });

        assert!(lhs.is_assignable_from(&rhs).is_err());
    }

    #[test]
    fn assign_type_var_to_bound_via_upper_bound() {
        use std::collections::HashMap;
        let id = next_type_id();
        let bounds = HashMap::from([(id, Type::Optional(Box::new(Type::Int())))]);
        assert!(
            crate::assign_type_with_bounds(
                &Type::Optional(Box::new(Type::Int())),
                &Type::Var(id),
                &bounds
            )
            .is_ok()
        );
    }

    #[test]
    fn assign_type_var_to_stricter_than_bound_fails() {
        use std::collections::HashMap;
        let id = next_type_id();
        let bounds = HashMap::from([(id, Type::Optional(Box::new(Type::Int())))]);
        assert!(crate::assign_type_with_bounds(&Type::Int(), &Type::Var(id), &bounds).is_err());
    }

    #[test]
    fn assign_type_var_with_record_bound_allows_field_access() {
        use std::collections::HashMap;
        let id = next_type_id();
        let mut record = RecordType::default();
        record.insert("x".to_string(), Type::Int());
        let bounds = HashMap::from([(id, Type::Record(record.clone()))]);
        assert!(
            crate::assign_type_with_bounds(&Type::Record(record), &Type::Var(id), &bounds).is_ok()
        );
    }

    #[test]
    fn assign_type_var_with_fn_bound_allows_fn_assignment() {
        use std::collections::HashMap;
        let id = next_type_id();
        let fn_ty = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Int()),
        });
        let bounds = HashMap::from([(id, fn_ty.clone())]);
        assert!(crate::assign_type_with_bounds(&fn_ty, &Type::Var(id), &bounds).is_ok());
    }

    // --- Type declaration tests ---

    fn check_module(source: &str) -> crate::Diagnosed<Type> {
        let module_id = ModuleId::default();
        let file_mod = crate::parser::parse_file_mod(source, &module_id).into_inner();
        let modules = Box::leak(Box::new(HashMap::new()));
        let checker = TypeChecker::from_modules(modules, Vec::new());
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
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
        assert_eq!(fn_ty.params[0], Type::Int());
        assert_eq!(*fn_ty.ret, Type::Int());
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
        let TypeKind::Record(param_rec) = &fn_ty.params[0].kind else {
            panic!("expected record param type, got: {}", fn_ty.params[0]);
        };
        assert_eq!(param_rec.get("fst"), Some(&Type::Int()));
        assert_eq!(param_rec.get("snd"), Some(&Type::Str()));
        assert_eq!(*fn_ty.ret, Type::Int());
    }

    #[test]
    fn forward_reference_between_type_defs() {
        let diagnosed = check_module("type B A\ntype A Int\nexport let f = fn(x: B) x");
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
        let fn_ty = get_export_fn(&diagnosed, "f");
        assert_eq!(fn_ty.params[0], Type::Int());
    }

    #[test]
    fn type_def_and_value_coexist() {
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
        let modules = Box::leak(Box::new(HashMap::new()));
        let checker = TypeChecker::from_modules(modules, Vec::new());
        let ge = super::GlobalTypeEnv::default();
        let env = super::TypeEnv::new(&ge).with_module_id(&module_id);
        let diagnosed = checker.type_level_exports(&env, &file_mod);
        assert!(!diagnosed.diags().has_errors());
        let type_exports = diagnosed.into_inner();
        let Some(config_ty) = type_exports.get("Config") else {
            panic!("expected exported type 'Config'");
        };
        let TypeKind::Record(config_rec) = &config_ty.kind else {
            panic!("expected record type");
        };
        assert_eq!(config_rec.get("host"), Some(&Type::Str()));
        assert_eq!(config_rec.get("port"), Some(&Type::Int()));
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
    fn recursive_non_fn_global_produces_cycle_error() {
        let diagnosed = check_module("export let node = { value: 1, child: node }");
        assert!(
            diagnosed.diags().has_errors(),
            "expected cyclic dependency error for non-function self-reference"
        );
    }

    #[test]
    fn recursive_fn_global_still_works() {
        let diagnosed = check_module("export let f = fn(n: Int) if (n == 0) 1 else f(n - 1)");
        assert!(
            !diagnosed.diags().has_errors(),
            "unexpected errors: {:?}",
            diagnosed.diags()
        );
    }

    #[test]
    fn recursive_global_non_recursive_body_simplifies() {
        let diagnosed = check_module("export let x = 42");
        assert!(!diagnosed.diags().has_errors());
        let TypeKind::Record(exports) = &diagnosed.as_ref().kind else {
            panic!("expected record type");
        };
        let x_ty = exports.get("x").expect("expected 'x' export");
        let unfolded = x_ty.unfold();
        assert_eq!(unfolded, Type::Int());
    }

    #[test]
    fn free_var_constraints_solve_basic() {
        let mut constraints = super::FreeVarConstraints::new();
        let primary_id = 100;
        let member_id = 101;
        constraints.register(primary_id);
        constraints.register(member_id);

        let mut record = RecordType::default();
        record.insert("value".into(), Type::Var(member_id));
        constraints.constrain(primary_id, Type::Record(record));

        let mut body_record = RecordType::default();
        body_record.insert("value".into(), Type::Int());
        body_record.insert("child".into(), Type::Var(primary_id));
        let body_type = Type::Record(body_record);

        let solved = constraints.solve(primary_id, &body_type);

        let member_solution = solved.iter().find(|(id, _)| *id == member_id);
        assert_eq!(
            member_solution.map(|(_, ty)| ty),
            Some(&Type::Int()),
            "member_id should resolve to Int via unification"
        );
    }

    #[test]
    fn free_var_constraints_solve_fn_return() {
        let mut constraints = super::FreeVarConstraints::new();
        let primary_id = 200;
        let ret_id = 201;
        constraints.register(primary_id);
        constraints.register(ret_id);

        constraints.constrain(
            primary_id,
            Type::Fn(FnType {
                type_params: vec![],
                params: vec![Type::Int()],
                ret: Box::new(Type::Var(ret_id)),
            }),
        );

        let body_type = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int()],
            ret: Box::new(Type::Str()),
        });

        let solved = constraints.solve(primary_id, &body_type);

        let ret_solution = solved.iter().find(|(id, _)| *id == ret_id);
        assert_eq!(
            ret_solution.map(|(_, ty)| ty),
            Some(&Type::Str()),
            "return type var should resolve to Str via unification"
        );
    }

    #[test]
    fn recursive_global_fn_if_branches_constrain_return() {
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
