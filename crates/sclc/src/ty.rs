use std::cell::RefCell;
use std::collections::BTreeMap;

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

#[derive(Clone, Debug)]
pub struct Type {
    pub kind: TypeKind,
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
        Self { kind, name: None }
    }

    pub fn named(kind: TypeKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: Some(name.into()),
        }
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
}

// Convenience constructors matching the old enum variants.
#[allow(non_upper_case_globals, non_snake_case)]
impl Type {
    pub const Any: Self = Self {
        kind: TypeKind::Any,
        name: None,
    };
    pub const Int: Self = Self {
        kind: TypeKind::Int,
        name: None,
    };
    pub const Float: Self = Self {
        kind: TypeKind::Float,
        name: None,
    };
    pub const Bool: Self = Self {
        kind: TypeKind::Bool,
        name: None,
    };
    pub const Str: Self = Self {
        kind: TypeKind::Str,
        name: None,
    };
    pub const Never: Self = Self {
        kind: TypeKind::Never,
        name: None,
    };

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Any,
    Int,
    Float,
    Bool,
    Str,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnType {
    /// Type parameter IDs paired with their upper bounds (defaults to Type::Any).
    pub type_params: Vec<(usize, Type)>,
    pub params: Vec<Type>,
    pub ret: Box<Type>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordType {
    fields: BTreeMap<String, Type>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictType {
    pub key: Box<Type>,
    pub value: Box<Type>,
}

impl RecordType {
    pub fn insert(&mut self, name: String, ty: Type) {
        self.fields.insert(name, ty);
    }

    pub fn get(&self, name: &str) -> Option<&Type> {
        self.fields.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Type)> {
        self.fields.iter()
    }

    fn map_types(&self, mut f: impl FnMut(&Type) -> Type) -> Self {
        let fields = self
            .fields
            .iter()
            .map(|(name, ty)| (name.clone(), f(ty)))
            .collect();
        Self { fields }
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
        // Preserve the display name from the original type.
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
                    // so just unwrap — preserving the body's name if present.
                    return *body.clone();
                }
                let rec = Type::IsoRec(*id, body.clone());
                body.substitute(&[(*id, rec)])
            }
            _ => self.clone(),
        }
    }
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
                    if *bound != Type::Any {
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
            type_params: vec![(10, Type::Any)],
            params: vec![Type::Var(10)],
            ret: Box::new(Type::Var(10)),
        });
        assert_eq!(ty.to_string(), "fn<A>(A) A");
    }

    #[test]
    fn display_generic_fn_two_params() {
        // fn<A, B>(A, B) A
        let ty = Type::Fn(FnType {
            type_params: vec![(5, Type::Any), (6, Type::Any)],
            params: vec![Type::Var(5), Type::Var(6)],
            ret: Box::new(Type::Var(5)),
        });
        assert_eq!(ty.to_string(), "fn<A, B>(A, B) A");
    }

    #[test]
    fn display_non_generic_fn() {
        let ty = Type::Fn(FnType {
            type_params: vec![],
            params: vec![Type::Int, Type::Str],
            ret: Box::new(Type::Bool),
        });
        assert_eq!(ty.to_string(), "fn(Int, Str) Bool");
    }

    #[test]
    fn display_generic_fn_with_complex_types() {
        // fn<A>(A, [A]) A?
        let ty = Type::Fn(FnType {
            type_params: vec![(42, Type::Any)],
            params: vec![Type::Var(42), Type::List(Box::new(Type::Var(42)))],
            ret: Box::new(Type::Optional(Box::new(Type::Var(42)))),
        });
        assert_eq!(ty.to_string(), "fn<A>(A, [A]) A?");
    }

    #[test]
    fn display_generic_fn_with_bound() {
        // fn<A <: Int>(A) A
        let ty = Type::Fn(FnType {
            type_params: vec![(10, Type::Int)],
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
            type_params: vec![(2, Type::Any)],
            params: vec![Type::Var(2)],
            ret: Box::new(Type::Var(1)),
        });
        let outer = Type::Fn(FnType {
            type_params: vec![(1, Type::Any)],
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
        let ty = Type::IsoRec(1, Box::new(Type::List(Box::new(Type::Int))));
        assert_eq!(ty.to_string(), "[Int]");
    }

    #[test]
    fn contains_var_basic() {
        assert!(Type::Var(5).contains_var(5));
        assert!(!Type::Var(5).contains_var(6));
        assert!(!Type::Int.contains_var(0));
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
            type_params: vec![(7, Type::Any)],
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
        let unnamed = Type::Int;
        assert_eq!(named, unnamed);
    }

    #[test]
    fn strip_name_removes_alias() {
        let ty = Type::named(TypeKind::Int, "UserId").strip_name();
        assert_eq!(ty.to_string(), "Int");
    }
}
