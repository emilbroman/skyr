use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::asg::RawModuleId;
use crate::loc::Span;

thread_local! {
    /// Stack of type parameter IDs currently being displayed. When a generic
    /// function type is formatted, its type-parameter IDs are pushed here so
    /// that nested `Type::Var` nodes can look up their index and print a
    /// friendly name (`A`, `B`, …) instead of a raw numeric ID.
    ///
    /// # Note for embedders
    ///
    /// This state is thread-local and only accessed during `Display::fmt`
    /// calls on [`Type`] / [`FnType`]. It is cleaned up after each
    /// formatting call completes, so it is safe to format types from any
    /// thread. However, formatting a `Type` inside a `Display` impl for
    /// another `Type` on the same thread is intentional — it is how nested
    /// generic types resolve their parameter names.
    static DISPLAY_TYPE_PARAMS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn typevar_name(index: usize) -> String {
    // 0 = A, 1 = B, … 25 = Z, 26 = A1, 27 = B1, …
    let letter = (b'A' + (index % 26) as u8) as char;
    let suffix = index / 26;
    if suffix == 0 {
        letter.to_string()
    } else {
        format!("{letter}{suffix}")
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Type {
    pub kind: TypeKind,
    /// Unique identifier tracking the *origin* of a value for propositional
    /// type refinement. Does NOT participate in equality or hashing.
    /// Fresh by default; reuse is explicit via `.with_id()`.
    #[serde(skip, default = "crate::checker::next_type_id")]
    id: usize,
    /// Optional display name from a source-level type alias.
    /// Does NOT participate in equality or hashing.
    name: Option<String>,
}

impl PartialEq for Type {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Type {}

impl Type {
    pub fn new(kind: TypeKind) -> Self {
        Self {
            kind,
            id: crate::checker::next_type_id(),
            name: None,
        }
    }

    pub fn named(kind: TypeKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            id: crate::checker::next_type_id(),
            name: Some(name.into()),
        }
    }

    /// Returns the unique TypeId for propositional refinement tracking.
    pub fn id(&self) -> usize {
        self.id
    }

    /// Reuse the TypeId from another type (explicit origin propagation).
    pub fn with_id(mut self, id: usize) -> Self {
        self.id = id;
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn strip_name(mut self) -> Self {
        self.name = None;
        self
    }

    pub fn has_name(&self) -> bool {
        self.name.is_some()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(AsRef::as_ref)
    }
}

// Convenience constructors matching the old enum variants.
#[allow(non_snake_case)]
impl Type {
    pub fn Any() -> Self {
        Self::new(TypeKind::Any)
    }
    pub fn Int() -> Self {
        Self::new(TypeKind::Int)
    }
    pub fn Float() -> Self {
        Self::new(TypeKind::Float)
    }
    pub fn Bool() -> Self {
        Self::new(TypeKind::Bool)
    }
    pub fn Str() -> Self {
        Self::new(TypeKind::Str)
    }
    pub fn Path() -> Self {
        Self::new(TypeKind::Path)
    }
    pub fn Never() -> Self {
        Self::new(TypeKind::Never)
    }

    pub fn Optional(inner: Box<Type>) -> Self {
        Self::new(TypeKind::Optional(inner))
    }
    pub fn List(inner: Box<Type>) -> Self {
        Self::new(TypeKind::List(inner))
    }
    pub fn Fn(fn_ty: FnType) -> Self {
        Self::new(TypeKind::Fn(fn_ty))
    }
    pub fn Record(record: RecordType) -> Self {
        Self::new(TypeKind::Record(record))
    }
    pub fn Dict(dict: DictType) -> Self {
        Self::new(TypeKind::Dict(dict))
    }
    pub fn IsoRec(id: usize, body: Box<Type>) -> Self {
        Self::new(TypeKind::IsoRec(id, body))
    }
    pub fn Var(id: usize) -> Self {
        Self::new(TypeKind::Var(id))
    }
    pub fn Exception(id: u64) -> Self {
        Self::new(TypeKind::Exception(id))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TypeKind {
    Any,
    Int,
    Float,
    Bool,
    Str,
    Path,
    Optional(Box<Type>),
    List(Box<Type>),
    Fn(FnType),
    Record(RecordType),
    Dict(DictType),
    IsoRec(usize, Box<Type>),
    Var(usize),
    Never,
    Exception(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FnType {
    /// Type parameter IDs paired with their upper bounds (defaults to `Type::Any()`).
    pub type_params: Vec<(usize, Type)>,
    pub params: Vec<Type>,
    pub ret: Box<Type>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RecordType {
    fields: BTreeMap<String, Type>,
    doc_comments: BTreeMap<String, String>,
    /// Source location where each field was declared, when known.
    /// Populated when the record type is derived from a `RecordTypeExpr`
    /// or a record literal expression. Used by the LSP for goto-definition
    /// and find-references on field references.
    ///
    /// Skipped during serialization: origins are in-memory metadata for the
    /// current compilation session only.
    #[serde(skip)]
    origins: BTreeMap<String, (RawModuleId, Span)>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DictType {
    pub key: Box<Type>,
    pub value: Box<Type>,
}

impl PartialEq for RecordType {
    fn eq(&self, other: &Self) -> bool {
        self.fields == other.fields
    }
}

impl Eq for RecordType {}

impl RecordType {
    pub fn insert(&mut self, name: String, ty: Type) {
        self.fields.insert(name, ty);
    }

    pub fn insert_with_meta(
        &mut self,
        name: String,
        ty: Type,
        doc: Option<String>,
        origin: Option<(RawModuleId, Span)>,
    ) {
        self.fields.insert(name.clone(), ty);
        if let Some(doc) = doc {
            self.doc_comments.insert(name.clone(), doc);
        }
        if let Some(origin) = origin {
            self.origins.insert(name, origin);
        }
    }

    pub fn insert_with_doc(&mut self, name: String, ty: Type, doc: Option<String>) {
        self.insert_with_meta(name, ty, doc, None);
    }

    pub fn get(&self, name: &str) -> Option<&Type> {
        self.fields.get(name)
    }

    pub fn get_doc(&self, name: &str) -> Option<&str> {
        self.doc_comments.get(name).map(|s| s.as_str())
    }

    pub fn get_origin(&self, name: &str) -> Option<&(RawModuleId, Span)> {
        self.origins.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Type)> {
        self.fields.iter()
    }

    pub(crate) fn map_types(&self, mut f: impl FnMut(&Type) -> Type) -> Self {
        let fields = self
            .fields
            .iter()
            .map(|(name, ty)| (name.clone(), f(ty)))
            .collect();
        Self {
            fields,
            doc_comments: self.doc_comments.clone(),
            origins: self.origins.clone(),
        }
    }
}

impl DictType {
    fn map_types(&self, mut f: impl FnMut(&Type) -> Type) -> Self {
        Self {
            key: Box::new(f(self.key.as_ref())),
            value: Box::new(f(self.value.as_ref())),
        }
    }
}

impl Type {
    /// Substitute multiple type variables at once.
    /// Display names are preserved on the outermost type — a named record like
    /// `X` keeps its alias through substitution as long as the type variable
    /// replacement doesn't change the named type itself. When a `Var` is
    /// directly replaced, the replacement's own name (or lack thereof) is used.
    pub fn substitute(&self, replacements: &[(usize, Type)]) -> Self {
        let result = match &self.kind {
            TypeKind::Var(id) => {
                for (target_id, replacement) in replacements {
                    if id == target_id {
                        return replacement.clone();
                    }
                }
                Type::Var(*id)
            }
            TypeKind::Any
            | TypeKind::Int
            | TypeKind::Float
            | TypeKind::Bool
            | TypeKind::Str
            | TypeKind::Path
            | TypeKind::Never => self.clone(),
            TypeKind::Exception(id) => Type::Exception(*id),
            TypeKind::Optional(ty) => Type::Optional(Box::new(ty.substitute(replacements))),
            TypeKind::List(ty) => Type::List(Box::new(ty.substitute(replacements))),
            TypeKind::Fn(fn_ty) => Type::Fn(FnType {
                type_params: fn_ty
                    .type_params
                    .iter()
                    .map(|(id, bound)| (*id, bound.substitute(replacements)))
                    .collect(),
                params: fn_ty
                    .params
                    .iter()
                    .map(|p| p.substitute(replacements))
                    .collect(),
                ret: Box::new(fn_ty.ret.substitute(replacements)),
            }),
            TypeKind::Record(record) => {
                Type::Record(record.map_types(|ty| ty.substitute(replacements)))
            }
            TypeKind::Dict(dict) => Type::Dict(dict.map_types(|ty| ty.substitute(replacements))),
            TypeKind::IsoRec(id, body) => {
                Type::IsoRec(*id, Box::new(body.substitute(replacements)))
            }
        };
        // Preserve the id and display name from the original type.
        let result = result.with_id(self.id);
        if let Some(name) = &self.name {
            result.with_name(name.clone())
        } else {
            result
        }
    }

    /// Returns `true` if this type contains `Type::Var(var_id)` anywhere.
    pub fn contains_var(&self, var_id: usize) -> bool {
        match &self.kind {
            TypeKind::Var(id) => *id == var_id,
            TypeKind::Any
            | TypeKind::Int
            | TypeKind::Float
            | TypeKind::Bool
            | TypeKind::Str
            | TypeKind::Path
            | TypeKind::Never => false,
            TypeKind::Exception(_) => false,
            TypeKind::Optional(ty) | TypeKind::List(ty) => ty.contains_var(var_id),
            TypeKind::Fn(fn_ty) => {
                fn_ty
                    .type_params
                    .iter()
                    .any(|(_, bound)| bound.contains_var(var_id))
                    || fn_ty.params.iter().any(|p| p.contains_var(var_id))
                    || fn_ty.ret.contains_var(var_id)
            }
            TypeKind::Record(record) => record.fields.values().any(|ty| ty.contains_var(var_id)),
            TypeKind::Dict(dict) => {
                dict.key.contains_var(var_id) || dict.value.contains_var(var_id)
            }
            TypeKind::IsoRec(id, body) => *id != var_id && body.contains_var(var_id),
        }
    }

    pub fn unfold(&self) -> Self {
        match &self.kind {
            TypeKind::IsoRec(id, body) => {
                if !body.contains_var(*id) {
                    // Non-recursive: body doesn't reference the bound variable,
                    // so just unwrap and continue unfolding in case of nested
                    // non-recursive IsoRec layers (e.g. re-exported globals).
                    return body.unfold();
                }
                let rec = Type::IsoRec(*id, body.clone());
                body.substitute(&[(*id, rec)])
            }
            _ => self.clone(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TypeError / TypeIssue
// ═══════════════════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════════════════
// Variance
// ═══════════════════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════════════════
// Subtyping
// ═══════════════════════════════════════════════════════════════════════════════

impl Type {
    pub fn is_assignable_from(&self, rhs: &Type) -> Result<(), TypeError> {
        assign_type_with_bounds(self, rhs, &HashMap::new())
    }

    pub fn is_disjoint_from(&self, other: &Type) -> bool {
        self.is_assignable_from(other).is_err() && other.is_assignable_from(self).is_err()
    }
}

pub(crate) fn assign_type_with_bounds(
    lhs: &Type,
    rhs: &Type,
    bounds: &HashMap<usize, Type>,
) -> Result<(), TypeError> {
    assign_type_impl(&lhs.unfold(), &rhs.unfold(), lhs, rhs, bounds)
}

fn assign_type_impl(
    lhs: &Type,
    rhs: &Type,
    orig_lhs: &Type,
    orig_rhs: &Type,
    bounds: &HashMap<usize, Type>,
) -> Result<(), TypeError> {
    if lhs == rhs || matches!(lhs.kind, TypeKind::Any) || matches!(rhs.kind, TypeKind::Never) {
        return Ok(());
    }

    let mismatch = || TypeIssue::Mismatch(orig_lhs.clone(), orig_rhs.clone());

    if let TypeKind::Optional(lhs_inner) = &lhs.kind {
        return match &rhs.kind {
            TypeKind::Optional(rhs_inner) => {
                assign_type_with_bounds(lhs_inner.as_ref(), rhs_inner.as_ref(), bounds)
                    .map_err(|err| err.causing(mismatch()))
            }
            TypeKind::Var(id)
                if bounds
                    .get(id)
                    .is_some_and(|b| matches!(b.kind, TypeKind::Optional(_))) =>
            {
                let upper_bound = &bounds[id];
                assign_type_with_bounds(lhs, upper_bound, bounds)
                    .map_err(|err| err.causing(mismatch()))
            }
            _ => assign_type_with_bounds(lhs_inner.as_ref(), rhs, bounds)
                .map_err(|err| err.causing(mismatch())),
        };
    }

    if let TypeKind::Var(id) = rhs.kind
        && let Some(upper_bound) = bounds.get(&id)
    {
        return assign_type_with_bounds(lhs, upper_bound, bounds)
            .map_err(|err| err.causing(mismatch()));
    }

    match &lhs.kind {
        TypeKind::Record(lhs_record) => match &rhs.kind {
            TypeKind::Record(rhs_record) => {
                for (name, lhs_field) in lhs_record.iter() {
                    let Some(rhs_field) = rhs_record.get(name) else {
                        if matches!(lhs_field.kind, TypeKind::Optional(_)) {
                            continue;
                        }
                        return Err(TypeError::new(mismatch()));
                    };
                    assign_type_with_bounds(lhs_field, rhs_field, bounds)
                        .map_err(|err| err.causing(mismatch()))?;
                }
                Ok(())
            }
            _ => Err(TypeError::new(mismatch())),
        },
        TypeKind::Dict(lhs_dict) => match &rhs.kind {
            TypeKind::Dict(rhs_dict) => {
                assign_type_with_bounds(lhs_dict.key.as_ref(), rhs_dict.key.as_ref(), bounds)
                    .map_err(|err| err.causing(mismatch()))?;
                assign_type_with_bounds(lhs_dict.value.as_ref(), rhs_dict.value.as_ref(), bounds)
                    .map_err(|err| err.causing(mismatch()))?;
                Ok(())
            }
            _ => Err(TypeError::new(mismatch())),
        },
        TypeKind::List(lhs_inner) => match &rhs.kind {
            TypeKind::List(rhs_inner) => {
                assign_type_with_bounds(lhs_inner.as_ref(), rhs_inner.as_ref(), bounds)
                    .map_err(|err| err.causing(mismatch()))
            }
            _ => Err(TypeError::new(mismatch())),
        },
        TypeKind::Fn(lhs_fn) => match &rhs.kind {
            TypeKind::Fn(rhs_fn) => {
                assign_fn_type(lhs_fn, rhs_fn, bounds).map_err(|err| err.causing(mismatch()))
            }
            _ => Err(TypeError::new(mismatch())),
        },
        _ => Err(TypeError::new(mismatch())),
    }
}

/// Check that a function type `rhs` is assignable to `lhs`.
///
/// Handles three cases:
/// 1. Both non-generic: direct structural check with contravariant params
/// 2. Generic rhs, non-generic lhs: unify to solve type params
/// 3. Both generic: F-sub rule with contravariant bounds and alpha-renaming
fn assign_fn_type(
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
        (true, true) => {
            for (lhs_param, rhs_param) in lhs_fn.params.iter().zip(rhs_fn.params.iter()) {
                assign_type_with_bounds(rhs_param, lhs_param, bounds)?;
            }
            assign_type_with_bounds(lhs_fn.ret.as_ref(), rhs_fn.ret.as_ref(), bounds)?;
            Ok(())
        }

        (true, false) => unify_generic_fn(lhs_fn, rhs_fn, bounds),

        (false, true) => {
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
            assign_fn_type(&instantiated_lhs, rhs_fn, bounds)
        }

        (false, false) => {
            if lhs_fn.type_params.len() != rhs_fn.type_params.len() {
                return Err(TypeError::new(TypeIssue::Mismatch(
                    Type::Fn(lhs_fn.clone()),
                    Type::Fn(rhs_fn.clone()),
                )));
            }

            // Quantifier bounds are contravariant (standard F<:): for
            // `∀α<:A. T <: ∀β<:B. U`, we require `B <: A`. Here `rhs`
            // is the source (A-side) and `lhs` the target (B-side), so
            // we check that the target bound is assignable into the
            // source bound.
            for ((_, lhs_bound), (_, rhs_bound)) in
                lhs_fn.type_params.iter().zip(rhs_fn.type_params.iter())
            {
                assign_type_with_bounds(rhs_bound, lhs_bound, bounds)?;
            }

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

            let mut extended_bounds = bounds.clone();
            for (id, bound) in &rhs_fn.type_params {
                extended_bounds.insert(*id, bound.clone());
            }

            assign_fn_type(&renamed_lhs, &body_rhs, &extended_bounds)
        }
    }
}

fn unify_generic_fn(
    lhs_fn: &FnType,
    rhs_fn: &FnType,
    bounds: &HashMap<usize, Type>,
) -> Result<(), TypeError> {
    let free_vars: HashSet<usize> = rhs_fn.type_params.iter().map(|(id, _)| *id).collect();

    let mut assertions: HashMap<usize, (Type, Type)> = rhs_fn
        .type_params
        .iter()
        .map(|(id, upper_bound)| (*id, (Type::Never(), upper_bound.clone())))
        .collect();

    for (lhs_param, rhs_param) in lhs_fn.params.iter().zip(rhs_fn.params.iter()) {
        collect_bounds(
            lhs_param,
            rhs_param,
            Variance::Contravariant,
            &free_vars,
            &mut assertions,
        )?;
    }

    collect_bounds(
        lhs_fn.ret.as_ref(),
        rhs_fn.ret.as_ref(),
        Variance::Covariant,
        &free_vars,
        &mut assertions,
    )?;

    for (lower, upper) in assertions.values() {
        assign_type_with_bounds(upper, lower, bounds).map_err(|err| {
            err.causing(TypeIssue::Mismatch(
                Type::Fn(lhs_fn.clone()),
                Type::Fn(rhs_fn.clone()),
            ))
        })?;
    }

    Ok(())
}

/// Infer type arguments for a generic function from the types of the arguments
/// at a call site. Uses contravariant bound collection on parameter types to
/// determine the tightest concrete type for each type variable, then validates
/// that each solution satisfies the declared upper bound.
///
/// Returns `Ok(replacements)` — one `(type_var_id, inferred_type)` per type
/// parameter — or `Err` if constraints are contradictory or bounds are violated.
pub fn infer_type_args(
    fn_ty: &FnType,
    arg_types: &[Type],
    bounds: &HashMap<usize, Type>,
) -> Result<Vec<(usize, Type)>, TypeError> {
    let free_vars: HashSet<usize> = fn_ty.type_params.iter().map(|(id, _)| *id).collect();

    let mut assertions: HashMap<usize, (Type, Type)> = fn_ty
        .type_params
        .iter()
        .map(|(id, upper_bound)| (*id, (Type::Never(), upper_bound.clone())))
        .collect();

    for (arg_ty, param_ty) in arg_types.iter().zip(fn_ty.params.iter()) {
        collect_bounds(
            arg_ty,
            param_ty,
            Variance::Contravariant,
            &free_vars,
            &mut assertions,
        )?;
    }

    // Check that each inferred lower bound satisfies the declared upper bound.
    for (lower, upper) in assertions.values() {
        assign_type_with_bounds(upper, lower, bounds)?;
    }

    Ok(fn_ty
        .type_params
        .iter()
        .map(|(id, _)| {
            let (lower, _) = assertions.get(id).unwrap();
            (*id, lower.clone())
        })
        .collect())
}

/// Walk two types structurally, collecting bounds for free type variables in rhs.
fn collect_bounds(
    lhs: &Type,
    rhs: &Type,
    variance: Variance,
    free_vars: &HashSet<usize>,
    assertions: &mut HashMap<usize, (Type, Type)>,
) -> Result<(), TypeError> {
    let lhs = &lhs.unfold();
    let rhs = &rhs.unfold();

    if let TypeKind::Var(id) = rhs.kind
        && free_vars.contains(&id)
    {
        let entry = assertions.get_mut(&id).expect("free var must have entry");
        match variance {
            Variance::Covariant => {
                tighten_upper(&mut entry.1, lhs)?;
            }
            Variance::Contravariant => {
                tighten_lower(&mut entry.0, lhs)?;
            }
        }
        return Ok(());
    }

    match (&lhs.kind, &rhs.kind) {
        (TypeKind::Optional(lhs_inner), TypeKind::Optional(rhs_inner)) => {
            collect_bounds(lhs_inner, rhs_inner, variance, free_vars, assertions)
        }
        (_, TypeKind::Optional(rhs_inner)) if variance == Variance::Covariant => {
            collect_bounds(lhs, rhs_inner, variance, free_vars, assertions)
        }
        (TypeKind::List(lhs_inner), TypeKind::List(rhs_inner)) => {
            collect_bounds(lhs_inner, rhs_inner, variance, free_vars, assertions)
        }
        (TypeKind::Record(lhs_record), TypeKind::Record(rhs_record)) => {
            for (name, rhs_field) in rhs_record.iter() {
                if let Some(lhs_field) = lhs_record.get(name) {
                    collect_bounds(lhs_field, rhs_field, variance, free_vars, assertions)?;
                }
            }
            Ok(())
        }
        (TypeKind::Dict(lhs_dict), TypeKind::Dict(rhs_dict)) => {
            collect_bounds(
                lhs_dict.key.as_ref(),
                rhs_dict.key.as_ref(),
                variance,
                free_vars,
                assertions,
            )?;
            collect_bounds(
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
            let flipped = variance.flip();
            for (lhs_param, rhs_param) in lhs_fn.params.iter().zip(rhs_fn.params.iter()) {
                collect_bounds(lhs_param, rhs_param, flipped, free_vars, assertions)?;
            }
            collect_bounds(
                lhs_fn.ret.as_ref(),
                rhs_fn.ret.as_ref(),
                variance,
                free_vars,
                assertions,
            )
        }
        _ => match variance {
            Variance::Covariant => lhs
                .is_assignable_from(rhs)
                .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
            Variance::Contravariant => rhs
                .is_assignable_from(lhs)
                .map_err(|err| err.causing(TypeIssue::Mismatch(lhs.clone(), rhs.clone()))),
        },
    }
}

fn tighten_upper(current: &mut Type, new_bound: &Type) -> Result<(), TypeError> {
    if current.is_assignable_from(new_bound).is_ok() {
        *current = new_bound.clone();
    } else if new_bound.is_assignable_from(current).is_ok() {
        // current is already tighter
    } else {
        return Err(TypeError::new(TypeIssue::Mismatch(
            current.clone(),
            new_bound.clone(),
        )));
    }
    Ok(())
}

fn tighten_lower(current: &mut Type, new_bound: &Type) -> Result<(), TypeError> {
    if new_bound.is_assignable_from(current).is_ok() {
        *current = new_bound.clone();
    } else if current.is_assignable_from(new_bound).is_ok() {
        // current is already tighter
    } else {
        // Incompatible lower bounds — their join is Any.
        *current = Type::Any();
    }
    Ok(())
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = &self.name {
            return write!(f, "{name}");
        }
        self.kind.fmt(f)
    }
}

impl std::fmt::Display for TypeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeKind::Any => write!(f, "Any"),
            TypeKind::Int => write!(f, "Int"),
            TypeKind::Float => write!(f, "Float"),
            TypeKind::Bool => write!(f, "Bool"),
            TypeKind::Str => write!(f, "Str"),
            TypeKind::Path => write!(f, "Path"),
            TypeKind::Optional(ty) => write!(f, "{ty}?"),
            TypeKind::List(ty) => write!(f, "[{ty}]"),
            TypeKind::Fn(fn_ty) => write!(f, "{fn_ty}"),
            TypeKind::Record(record) => write!(f, "{record}"),
            TypeKind::Dict(dict) => write!(f, "{dict}"),
            TypeKind::IsoRec(id, ty) => {
                if !ty.contains_var(*id) {
                    return write!(f, "{ty}");
                }

                let name = DISPLAY_TYPE_PARAMS.with(|stack| {
                    let mut stack = stack.borrow_mut();
                    let name = typevar_name(stack.len());
                    stack.push(*id);
                    name
                });

                write!(f, "µ{name}.{ty}")?;

                DISPLAY_TYPE_PARAMS.with(|stack| {
                    let mut s = stack.borrow_mut();
                    s.pop();
                });

                Ok(())
            }
            TypeKind::Var(id) => {
                let name = DISPLAY_TYPE_PARAMS.with(|stack| {
                    stack
                        .borrow()
                        .iter()
                        .position(|v| v == id)
                        .map(typevar_name)
                });
                match name {
                    Some(name) => write!(f, "{name}"),
                    None => write!(f, "T{id}"),
                }
            }
            TypeKind::Never => write!(f, "Never"),
            TypeKind::Exception(id) => write!(f, "Exception#{id}"),
        }
    }
}

impl std::fmt::Display for FnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fn")?;

        let param_count = self.type_params.len();

        // Push type-param IDs so nested Var nodes resolve to friendly names.
        if param_count > 0 {
            DISPLAY_TYPE_PARAMS.with(|stack| {
                stack
                    .borrow_mut()
                    .extend(self.type_params.iter().map(|(id, _)| *id));
            });
        }

        // Helper closure so we can use `?` and still guarantee cleanup.
        let result = (|| {
            if param_count > 0 {
                write!(f, "<")?;
                for (i, (id, bound)) in self.type_params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    // Look up the name we just registered.
                    let name = DISPLAY_TYPE_PARAMS.with(|stack| {
                        stack
                            .borrow()
                            .iter()
                            .position(|v| v == id)
                            .map(typevar_name)
                            .unwrap_or_else(|| format!("T{id}"))
                    });
                    write!(f, "{name}")?;
                    if *bound != Type::Any() {
                        write!(f, " <: {bound}")?;
                    }
                }
                write!(f, ">")?;
            }

            write!(f, "(")?;

            let mut params = self.params.iter().peekable();
            while let Some(param) = params.next() {
                write!(f, "{param}")?;
                if params.peek().is_some() {
                    write!(f, ", ")?;
                }
            }

            write!(f, ") {}", self.ret)
        })();

        // Pop the type-param IDs we pushed, regardless of success/failure.
        if param_count > 0 {
            DISPLAY_TYPE_PARAMS.with(|stack| {
                let mut s = stack.borrow_mut();
                let new_len = s.len() - param_count;
                s.truncate(new_len);
            });
        }

        result
    }
}

impl std::fmt::Display for RecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{")?;

        let mut fields = self.fields.iter().peekable();
        while let Some((name, ty)) = fields.next() {
            write!(f, "{name}: {ty}")?;
            if fields.peek().is_some() {
                write!(f, ", ")?;
            }
        }

        write!(f, "}}")
    }
}

impl std::fmt::Display for DictType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{{{}: {}}}", self.key, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typevar_name_letters() {
        assert_eq!(typevar_name(0), "A");
        assert_eq!(typevar_name(1), "B");
        assert_eq!(typevar_name(25), "Z");
        assert_eq!(typevar_name(26), "A1");
        assert_eq!(typevar_name(27), "B1");
        assert_eq!(typevar_name(52), "A2");
    }

    #[test]
    fn display_var_without_scope_falls_back() {
        let ty = Type::Var(99);
        assert_eq!(ty.to_string(), "T99");
    }

    #[test]
    fn display_generic_fn_single_param() {
        // fn<A>(A) A
        let ty = Type::Fn(FnType {
            type_params: vec![(10, Type::Any())],
            params: vec![Type::Var(10)],
            ret: Box::new(Type::Var(10)),
        });
        assert_eq!(ty.to_string(), "fn<A>(A) A");
    }

    #[test]
    fn display_generic_fn_two_params() {
        // fn<A, B>(A, B) A
        let ty = Type::Fn(FnType {
            type_params: vec![(5, Type::Any()), (6, Type::Any())],
            params: vec![Type::Var(5), Type::Var(6)],
            ret: Box::new(Type::Var(5)),
        });
        assert_eq!(ty.to_string(), "fn<A, B>(A, B) A");
    }

    #[test]
    fn display_non_generic_fn() {
        let ty = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int(), Type::Str()],
            ret: Box::new(Type::Bool()),
        });
        assert_eq!(ty.to_string(), "fn(Int, Str) Bool");
    }

    #[test]
    fn display_generic_fn_with_complex_types() {
        // fn<A>(A, [A]) A?
        let ty = Type::Fn(FnType {
            type_params: vec![(42, Type::Any())],
            params: vec![Type::Var(42), Type::List(Box::new(Type::Var(42)))],
            ret: Box::new(Type::Optional(Box::new(Type::Var(42)))),
        });
        assert_eq!(ty.to_string(), "fn<A>(A, [A]) A?");
    }

    #[test]
    fn display_generic_fn_with_bound() {
        // fn<A <: Int>(A) A
        let ty = Type::Fn(FnType {
            type_params: vec![(10, Type::Int())],
            params: vec![Type::Var(10)],
            ret: Box::new(Type::Var(10)),
        });
        assert_eq!(ty.to_string(), "fn<A <: Int>(A) A");
    }

    #[test]
    fn display_nested_generic_fns() {
        // Outer fn has type param id=1, inner has id=2.
        // When outer formats: stack = [1], so Var(1) = A.
        // When inner formats: stack = [1, 2], so Var(2) = B, Var(1) = A.
        let inner = Type::Fn(FnType {
            type_params: vec![(2, Type::Any())],
            params: vec![Type::Var(2)],
            ret: Box::new(Type::Var(1)),
        });
        let outer = Type::Fn(FnType {
            type_params: vec![(1, Type::Any())],
            params: vec![Type::Var(1)],
            ret: Box::new(inner),
        });
        assert_eq!(outer.to_string(), "fn<A>(A) fn<B>(B) A");
    }

    #[test]
    fn display_isorec_recursive() {
        // µA.[A] — the var is used, so we keep the µ binder
        let ty = Type::IsoRec(1, Box::new(Type::List(Box::new(Type::Var(1)))));
        assert_eq!(ty.to_string(), "µA.[A]");
    }

    #[test]
    fn display_isorec_non_recursive() {
        // IsoRec where the var is NOT used — should simplify to just the body
        let ty = Type::IsoRec(1, Box::new(Type::List(Box::new(Type::Int()))));
        assert_eq!(ty.to_string(), "[Int]");
    }

    #[test]
    fn contains_var_basic() {
        assert!(Type::Var(5).contains_var(5));
        assert!(!Type::Var(5).contains_var(6));
        assert!(!Type::Int().contains_var(0));
        assert!(Type::List(Box::new(Type::Var(3))).contains_var(3));
        assert!(!Type::List(Box::new(Type::Var(3))).contains_var(4));
    }

    #[test]
    fn contains_var_shadowed_by_isorec() {
        // IsoRec(1, Var(1)) — the inner Var is bound by the IsoRec, not free
        let ty = Type::IsoRec(1, Box::new(Type::Var(1)));
        assert!(!ty.contains_var(1));
    }

    #[test]
    fn display_stack_cleaned_up_after_formatting() {
        // Format a generic fn, then check that a bare Var falls back.
        let generic = Type::Fn(FnType {
            type_params: vec![(7, Type::Any())],
            params: vec![Type::Var(7)],
            ret: Box::new(Type::Var(7)),
        });
        assert_eq!(generic.to_string(), "fn<A>(A) A");

        let bare = Type::Var(7);
        assert_eq!(bare.to_string(), "T7");
    }

    #[test]
    fn display_named_type_uses_name() {
        let ty = Type::named(TypeKind::Int, "UserId");
        assert_eq!(ty.to_string(), "UserId");
    }

    #[test]
    fn display_unnamed_type_uses_kind() {
        let ty = Type::new(TypeKind::Int);
        assert_eq!(ty.to_string(), "Int");
    }

    #[test]
    fn named_type_equals_unnamed() {
        let named = Type::named(TypeKind::Int, "UserId");
        let unnamed = Type::Int();
        assert_eq!(named, unnamed);
    }

    #[test]
    fn strip_name_removes_alias() {
        let ty = Type::named(TypeKind::Int, "UserId").strip_name();
        assert_eq!(ty.to_string(), "Int");
    }
}
