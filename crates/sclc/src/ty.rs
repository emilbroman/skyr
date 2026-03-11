use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    /// Stack of type parameter IDs currently being displayed. When a generic
    /// function type is formatted, its type-parameter IDs are pushed here so
    /// that nested `Type::Var` nodes can look up their index and print a
    /// friendly name (`A`, `B`, …) instead of a raw numeric ID.
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
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
    pub fn substitute(&self, replacements: &[(usize, Type)]) -> Self {
        match self {
            Type::Var(id) => {
                for (target_id, replacement) in replacements {
                    if id == target_id {
                        return replacement.clone();
                    }
                }
                Type::Var(*id)
            }
            Type::Any | Type::Int | Type::Float | Type::Bool | Type::Str | Type::Never => {
                self.clone()
            }
            Type::Exception(id) => Type::Exception(*id),
            Type::Optional(ty) => Type::Optional(Box::new(ty.substitute(replacements))),
            Type::List(ty) => Type::List(Box::new(ty.substitute(replacements))),
            Type::Fn(fn_ty) => Type::Fn(FnType {
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
            Type::Record(record) => {
                Type::Record(record.map_types(|ty| ty.substitute(replacements)))
            }
            Type::Dict(dict) => Type::Dict(dict.map_types(|ty| ty.substitute(replacements))),
            Type::IsoRec(id, body) => Type::IsoRec(*id, Box::new(body.substitute(replacements))),
        }
    }

    pub fn unfold(&self) -> Self {
        match self {
            Type::IsoRec(id, body) => {
                let rec = Type::IsoRec(*id, body.clone());
                body.substitute(&[(*id, rec)])
            }
            _ => self.clone(),
        }
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Any => write!(f, "Any"),
            Type::Int => write!(f, "Int"),
            Type::Float => write!(f, "Float"),
            Type::Bool => write!(f, "Bool"),
            Type::Str => write!(f, "Str"),
            Type::Optional(ty) => write!(f, "{ty}?"),
            Type::List(ty) => write!(f, "[{ty}]"),
            Type::Fn(fn_ty) => write!(f, "{fn_ty}"),
            Type::Record(record) => write!(f, "{record}"),
            Type::Dict(dict) => write!(f, "{dict}"),
            Type::IsoRec(id, ty) => {
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
            Type::Var(id) => {
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
            Type::Never => write!(f, "Never"),
            Type::Exception(id) => write!(f, "Exception#{id}"),
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
}
