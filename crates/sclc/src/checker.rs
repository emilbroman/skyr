use std::collections::{HashMap, HashSet};
use std::path::Component;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    AnySource, DiagList, Diagnosed, DictType, FnType, Package, Program, RecordType, Type, ast,
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

pub struct TypeEnv<'a> {
    module_id: Option<&'a crate::ModuleId>,
    globals: Option<&'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>>,
    imports: Option<&'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>>,
    locals: HashMap<&'a str, Type>,
    type_vars: HashMap<String, Type>,
    /// Upper bounds for type variable IDs (used during function body checking).
    type_var_bounds: HashMap<usize, Type>,
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
            locals: HashMap::new(),
            type_vars: HashMap::new(),
            type_var_bounds: HashMap::new(),
        }
    }

    pub fn inner(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            type_vars: self.type_vars.clone(),
            type_var_bounds: self.type_var_bounds.clone(),
        }
    }

    pub fn with_globals(&self, globals: &'a HashMap<&'a str, &'a crate::Loc<ast::Expr>>) -> Self {
        Self {
            module_id: self.module_id,
            globals: Some(globals),
            imports: self.imports,
            locals: HashMap::new(),
            type_vars: self.type_vars.clone(),
            type_var_bounds: self.type_var_bounds.clone(),
        }
    }

    pub fn with_imports(
        &self,
        imports: &'a HashMap<&'a str, (crate::ModuleId, Option<&'a ast::FileMod>)>,
    ) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: Some(imports),
            locals: HashMap::new(),
            type_vars: self.type_vars.clone(),
            type_var_bounds: self.type_var_bounds.clone(),
        }
    }

    pub fn with_module_id(&self, module_id: &'a crate::ModuleId) -> Self {
        Self {
            module_id: Some(module_id),
            globals: self.globals,
            imports: self.imports,
            locals: self.locals.clone(),
            type_vars: self.type_vars.clone(),
            type_var_bounds: self.type_var_bounds.clone(),
        }
    }

    pub fn with_local(&self, name: &'a str, ty: Type) -> Self {
        let mut env = self.inner();
        env.locals.insert(name, ty);
        env
    }

    pub fn with_type_var(&self, name: String, ty: Type) -> Self {
        let mut env = self.inner();
        env.type_vars.insert(name, ty);
        env
    }

    pub fn with_type_var_bound(&self, id: usize, upper_bound: Type) -> Self {
        let mut env = self.inner();
        env.type_var_bounds.insert(id, upper_bound);
        env
    }

    /// If `ty` is a type variable with a known upper bound, return a reference
    /// to the bound. Otherwise, return the passed-in reference unchanged.
    pub fn resolve_var_bound<'t>(&'t self, ty: &'t Type) -> &'t Type {
        if let Type::Var(id) = ty
            && let Some(bound) = self.type_var_bounds.get(id)
        {
            return bound;
        }
        ty
    }

    pub fn without_locals(&self) -> Self {
        Self {
            module_id: self.module_id,
            globals: self.globals,
            imports: self.imports,
            locals: HashMap::new(),
            type_vars: self.type_vars.clone(),
            type_var_bounds: self.type_var_bounds.clone(),
        }
    }

    pub fn lookup_type_var(&self, name: &str) -> Option<&Type> {
        self.type_vars.get(name)
    }

    pub fn lookup_local(&self, name: &str) -> Option<&Type> {
        self.locals.get(name)
    }

    pub fn lookup_global(&self, name: &str) -> Option<&crate::Loc<ast::Expr>> {
        self.globals.and_then(|globals| globals.get(name).copied())
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
}

pub struct TypeChecker<'p, S> {
    program: &'p Program<S>,
}

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

        if lhs == rhs || matches!(lhs, Type::Any) || matches!(rhs, Type::Never) {
            return Ok(());
        }

        // If rhs is a bounded type variable, check assignability via its upper bound.
        if let Type::Var(id) = rhs
            && let Some(upper_bound) = bounds.get(id)
        {
            return self
                .assign_type_with_bounds(lhs, upper_bound, bounds)
                .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone())));
        }

        match lhs {
            Type::Optional(lhs_inner) => match rhs {
                Type::Optional(rhs_inner) => self
                    .assign_type_with_bounds(lhs_inner.as_ref(), rhs_inner.as_ref(), bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
                _ => self
                    .assign_type_with_bounds(lhs_inner.as_ref(), rhs, bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
            },
            Type::Record(lhs_record) => match rhs {
                Type::Record(rhs_record) => {
                    for (name, lhs_field) in lhs_record.iter() {
                        let Some(rhs_field) = rhs_record.get(name) else {
                            if matches!(lhs_field, Type::Optional(_)) {
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
            Type::Dict(lhs_dict) => match rhs {
                Type::Dict(rhs_dict) => {
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
            Type::List(lhs_inner) => match rhs {
                Type::List(rhs_inner) => self
                    .assign_type_with_bounds(lhs_inner.as_ref(), rhs_inner.as_ref(), bounds)
                    .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
                _ => Err(TypeError::new(TypeIssue::Mismatch(
                    lhs.clone(),
                    rhs.clone(),
                ))),
            },
            Type::Fn(lhs_fn) => match rhs {
                Type::Fn(rhs_fn) => self
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
        if let Type::Var(id) = rhs
            && free_vars.contains(id)
        {
            let entry = assertions.get_mut(id).expect("free var must have entry");
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
        match (lhs, rhs) {
            (Type::Optional(lhs_inner), Type::Optional(rhs_inner)) => {
                self.collect_bounds(lhs_inner, rhs_inner, variance, free_vars, assertions)
            }
            (_, Type::Optional(rhs_inner)) if variance == Variance::Covariant => {
                // Non-optional lhs can be assigned to optional rhs in covariant position
                self.collect_bounds(lhs, rhs_inner, variance, free_vars, assertions)
            }
            (Type::List(lhs_inner), Type::List(rhs_inner)) => {
                self.collect_bounds(lhs_inner, rhs_inner, variance, free_vars, assertions)
            }
            (Type::Record(lhs_record), Type::Record(rhs_record)) => {
                for (name, rhs_field) in rhs_record.iter() {
                    if let Some(lhs_field) = lhs_record.get(name) {
                        self.collect_bounds(lhs_field, rhs_field, variance, free_vars, assertions)?;
                    }
                }
                Ok(())
            }
            (Type::Dict(lhs_dict), Type::Dict(rhs_dict)) => {
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
            (Type::Fn(lhs_fn), Type::Fn(rhs_fn)) if lhs_fn.params.len() == rhs_fn.params.len() => {
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
        if let Some(expected_type) = expected_type
            && let Err(error) =
                self.assign_type_with_bounds(expected_type, &ty, &env.type_var_bounds)
        {
            diags.push(InvalidType {
                module_id: env.module_id()?,
                error,
                span,
            });
        }

        Ok(Diagnosed::new(ty, diags))
    }

    pub fn new(program: &'p Program<S>) -> Self {
        Self { program }
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

    pub fn check_file_mod(
        &self,
        env: &TypeEnv<'_>,
        file_mod: &ast::FileMod,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let globals = file_mod.find_globals();
        let imports = self.find_imports(file_mod);
        let env = env.with_globals(&globals).with_imports(&imports);

        let mut diags = DiagList::new();
        let mut exports = RecordType::default();

        for statement in &file_mod.statements {
            if let Some((name, ty)) = self.check_stmt(&env, statement)?.unpack(&mut diags) {
                exports.insert(name, ty);
            }
        }

        Ok(Diagnosed::new(Type::Record(exports), diags))
    }

    pub fn check_stmt(
        &self,
        env: &TypeEnv<'_>,
        stmt: &ast::ModStmt,
    ) -> Result<Diagnosed<Option<(String, Type)>>, TypeCheckError> {
        match stmt {
            ast::ModStmt::Import(_) => Ok(Diagnosed::new(None, DiagList::new())),
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
        }
    }

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
                let bind_ty = self
                    .check_expr(env, let_expr.bind.expr.as_ref(), None)?
                    .unpack(&mut diags);
                let inner_env = env.with_local(let_expr.bind.var.name.as_str(), bind_ty);
                let body_ty = self
                    .check_expr(&inner_env, let_expr.expr.as_ref(), expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(body_ty, diags))
            }
            ast::Expr::Fn(fn_expr) => {
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
                    fn_env = fn_env.with_local(param.var.name.as_str(), param_ty.clone());
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
            ast::Expr::Call(call_expr) => {
                let mut diags = DiagList::new();
                let raw_callee_ty = self
                    .check_expr(env, call_expr.callee.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                let callee_ty = env.resolve_var_bound(&raw_callee_ty).unfold();
                if matches!(callee_ty, Type::Never) {
                    return Ok(Diagnosed::new(Type::Never, diags));
                }
                let Type::Fn(fn_ty) = callee_ty else {
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
                                let resolved =
                                    self.resolve_type_expr(env, type_arg).unpack(&mut diags);
                                // Check that the type argument satisfies the declared bound
                                if self
                                    .assign_type_with_bounds(bound, &resolved, &env.type_var_bounds)
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
            ast::Expr::Unary(unary_expr) => {
                let mut diags = DiagList::new();
                let operand_ty = self
                    .check_expr(env, unary_expr.expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();

                let result_ty = if matches!(operand_ty, Type::Never) {
                    Type::Never
                } else {
                    match unary_expr.op {
                        ast::UnaryOp::Negate => match operand_ty {
                            Type::Int => Type::Int,
                            Type::Float => Type::Float,
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

                let result_ty = if matches!(lhs_ty, Type::Never) || matches!(rhs_ty, Type::Never) {
                    Type::Never
                } else {
                    match binary_expr.op {
                        ast::BinaryOp::Add => match (&lhs_ty, &rhs_ty) {
                            (Type::Int, Type::Int) => Type::Int,
                            (Type::Float, Type::Float) => Type::Float,
                            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
                            (Type::Str, Type::Str) => Type::Str,
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
                        ast::BinaryOp::Sub => match (&lhs_ty, &rhs_ty) {
                            (Type::Int, Type::Int) => Type::Int,
                            (Type::Float, Type::Float) => Type::Float,
                            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
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
                        ast::BinaryOp::Mul => match (&lhs_ty, &rhs_ty) {
                            (Type::Int, Type::Int) => Type::Int,
                            (Type::Float, Type::Float) => Type::Float,
                            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
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
                        ast::BinaryOp::Div => match (&lhs_ty, &rhs_ty) {
                            (Type::Int, Type::Int) => Type::Int,
                            (Type::Float, Type::Float) => Type::Float,
                            (Type::Int, Type::Float) | (Type::Float, Type::Int) => Type::Float,
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
                        | ast::BinaryOp::Gte => match (&lhs_ty, &rhs_ty) {
                            (Type::Int, Type::Int)
                            | (Type::Float, Type::Float)
                            | (Type::Int, Type::Float)
                            | (Type::Float, Type::Int) => Type::Bool,
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
                        ast::BinaryOp::And | ast::BinaryOp::Or => match (&lhs_ty, &rhs_ty) {
                            (Type::Bool, Type::Bool) => Type::Bool,
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
                    }
                };

                let ty = self
                    .apply_expected_type(env, expr.span(), result_ty, expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(ty, diags))
            }
            ast::Expr::Var(var) => {
                if let Some(local_ty) = env.lookup_local(var.name.as_str()) {
                    return self.apply_expected_type(
                        env,
                        expr.span(),
                        local_ty.clone(),
                        expected_type,
                    );
                }
                if let Some(global_expr) = env.lookup_global(var.name.as_str()) {
                    let mut diags = DiagList::new();
                    let type_id = next_type_id();
                    let global_env = env
                        .without_locals()
                        .with_local(var.name.as_str(), Type::Var(type_id));
                    let resolved_ty = self
                        .check_expr(&global_env, global_expr, expected_type)?
                        .unpack(&mut diags);
                    let ty = self
                        .apply_expected_type(
                            env,
                            expr.span(),
                            Type::IsoRec(type_id, Box::new(resolved_ty)),
                            expected_type,
                        )?
                        .unpack(&mut diags);
                    return Ok(Diagnosed::new(ty, diags));
                }
                if let Some((target_module_id, maybe_import_file_mod)) =
                    env.lookup_import(var.name.as_str())
                {
                    let Some(import_file_mod) = maybe_import_file_mod else {
                        return Ok(Diagnosed::new(Type::Never, DiagList::new()));
                    };
                    let import_env = TypeEnv::new().with_module_id(&target_module_id);
                    let imported_ty = self.check_file_mod(&import_env, import_file_mod)?;
                    return self.apply_expected_type(
                        env,
                        expr.span(),
                        imported_ty.into_inner(),
                        expected_type,
                    );
                }
                let mut diags = DiagList::new();
                diags.push(UndefinedVariable {
                    module_id: env.module_id()?,
                    name: var.name.clone(),
                    var: var.clone(),
                });
                Ok(Diagnosed::new(Type::Never, diags))
            }
            ast::Expr::Record(record_expr) => {
                let mut diags = DiagList::new();
                let mut record_ty = RecordType::default();
                let expected_record = match expected_type {
                    Some(Type::Record(record_ty)) => Some(record_ty),
                    _ => None,
                };

                for field in &record_expr.fields {
                    let expected_field_ty = expected_record
                        .and_then(|record_ty| record_ty.get(field.var.name.as_str()));
                    let field_ty = self
                        .check_expr(env, &field.expr, expected_field_ty)?
                        .unpack(&mut diags);
                    record_ty.insert(field.var.name.clone(), field_ty);
                }
                let ty = Type::Record(record_ty);
                if let Some(expected_record) = expected_record {
                    let missing_field = expected_record.iter().any(|(name, field_ty)| {
                        matches!(ty, Type::Record(ref record) if record.get(name).is_none())
                            && !matches!(field_ty, Type::Optional(_))
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
                    Some(Type::Dict(dict_ty)) => Some(dict_ty),
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
                let list_ty = if let Some(Type::List(expected_item_ty)) = expected_type {
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
            ast::Expr::PropertyAccess(property_access) => {
                let mut diags = DiagList::new();
                let raw_lhs_ty = self
                    .check_expr(env, property_access.expr.as_ref(), None)?
                    .unpack(&mut diags)
                    .unfold();
                let lhs_ty = env.resolve_var_bound(&raw_lhs_ty).unfold();
                if matches!(lhs_ty, Type::Never) {
                    return Ok(Diagnosed::new(Type::Never, diags));
                }
                let member_ty = match &lhs_ty {
                    Type::Record(record_ty) => record_ty
                        .get(property_access.property.name.as_str())
                        .cloned(),
                    _ => None,
                };
                if let Some(member_ty) = member_ty {
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
                if !matches!(inner_ty, Type::Exception(_) | Type::Never) {
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

                    match &catch_var_ty {
                        Type::Exception(_) => {
                            if let Some(catch_arg) = &catch.catch_arg {
                                diags.push(UnexpectedCatchArg {
                                    module_id: env.module_id()?,
                                    span: catch_arg.span(),
                                });
                            }
                            self.check_expr(env, &catch.body, Some(&try_ty))?
                                .unpack(&mut diags);
                        }
                        Type::Fn(fn_ty) => {
                            let ret_ty = fn_ty.ret.as_ref().clone().unfold();
                            if !matches!(ret_ty, Type::Exception(_)) {
                                diags.push(InvalidCatchTarget {
                                    module_id: env.module_id()?,
                                    ty: catch_var_ty.clone(),
                                    span: catch.exception_var.span(),
                                });
                            }
                            if let Some(catch_arg) = &catch.catch_arg {
                                let param_ty = fn_ty.params.first().cloned().unwrap_or(Type::Never);
                                let inner_env = env.with_local(catch_arg.name.as_str(), param_ty);
                                self.check_expr(&inner_env, &catch.body, Some(&try_ty))?
                                    .unpack(&mut diags);
                            } else {
                                self.check_expr(env, &catch.body, Some(&try_ty))?
                                    .unpack(&mut diags);
                            }
                        }
                        Type::Never => {
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
                let element_ty = match iterable_ty.clone() {
                    Type::List(element_ty) => *element_ty,
                    other => {
                        diags.push(InvalidType {
                            module_id: env.module_id()?,
                            error: TypeError::new(TypeIssue::Mismatch(
                                Type::List(Box::new(Type::Any)),
                                other,
                            )),
                            span: for_item.iterable.span(),
                        });
                        Type::Never
                    }
                };
                let inner_env = env.with_local(for_item.var.name.as_str(), element_ty);
                let item_ty = self
                    .check_list_item(&inner_env, for_item.emit_item.as_ref(), expected_type)?
                    .unpack(&mut diags);
                Ok(Diagnosed::new(item_ty, diags))
            }
        }
    }

    fn check_global_let_bind(
        &self,
        env: &TypeEnv<'_>,
        let_bind: &ast::LetBind,
    ) -> Result<Diagnosed<Type>, TypeCheckError> {
        let mut diags = DiagList::new();
        let type_id = next_type_id();
        let env = env.with_local(let_bind.var.name.as_str(), Type::Var(type_id));
        let resolved_ty = self
            .check_expr(&env, let_bind.expr.as_ref(), None)?
            .unpack(&mut diags);
        Ok(Diagnosed::new(
            Type::IsoRec(type_id, Box::new(resolved_ty)),
            diags,
        ))
    }

    fn resolve_type_expr(
        &self,
        env: &TypeEnv<'_>,
        type_expr: &crate::Loc<ast::TypeExpr>,
    ) -> Diagnosed<Type> {
        let mut diags = DiagList::new();
        let ty = match type_expr.as_ref() {
            ast::TypeExpr::Var(var) if var.name == "Any" => Type::Any,
            ast::TypeExpr::Var(var) if var.name == "Int" => Type::Int,
            ast::TypeExpr::Var(var) if var.name == "Float" => Type::Float,
            ast::TypeExpr::Var(var) if var.name == "Bool" => Type::Bool,
            ast::TypeExpr::Var(var) if var.name == "Str" => Type::Str,
            ast::TypeExpr::Var(var) => {
                if let Some(ty) = env.lookup_type_var(var.name.as_str()) {
                    ty.clone()
                } else {
                    if let Ok(module_id) = env.module_id() {
                        diags.push(UnknownType {
                            module_id,
                            name: var.name.clone(),
                            span: type_expr.span(),
                        });
                    }
                    Type::Never
                }
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
        };
        Diagnosed::new(ty, diags)
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
                    var: loc(Var { name: "a".into() }, span),
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
                    var: loc(Var { name: "a".into() }, field_span),
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
        assert!(matches!(*diagnosed, Type::Never));

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
        assert!(matches!(*diagnosed, Type::Never));
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
        assert!(matches!(*diagnosed, Type::Never));
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
}
