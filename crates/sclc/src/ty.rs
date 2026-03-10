use std::collections::BTreeMap;

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
    pub type_params: Vec<usize>,
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
                type_params: fn_ty.type_params.clone(),
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
            Type::IsoRec(id, ty) => write!(f, "IsoRec({id}, {ty})"),
            Type::Var(id) => write!(f, "T{id}"),
            Type::Never => write!(f, "Never"),
            Type::Exception(id) => write!(f, "Exception#{id}"),
        }
    }
}

impl std::fmt::Display for FnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fn")?;

        if !self.type_params.is_empty() {
            write!(f, "<")?;
            let mut type_params = self.type_params.iter().peekable();
            while let Some(id) = type_params.next() {
                write!(f, "T{id}")?;
                if type_params.peek().is_some() {
                    write!(f, ", ")?;
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
