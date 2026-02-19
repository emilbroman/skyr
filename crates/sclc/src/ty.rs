use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
    Str,
    Optional(Box<Type>),
    List(Box<Type>),
    Fn(FnType),
    Record(RecordType),
    IsoRec(usize, Box<Type>),
    Var(usize),
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnType {
    pub params: Vec<Type>,
    pub ret: Box<Type>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordType {
    fields: BTreeMap<String, Type>,
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

impl Type {
    pub fn unfold(&self) -> Self {
        self.unfold_inner(None)
    }

    fn unfold_inner(&self, replacement: Option<(usize, &Type)>) -> Self {
        match self {
            Type::Int => Type::Int,
            Type::Bool => Type::Bool,
            Type::Str => Type::Str,
            Type::Optional(ty) => Type::Optional(Box::new(ty.unfold_inner(replacement))),
            Type::List(ty) => Type::List(Box::new(ty.unfold_inner(replacement))),
            Type::Fn(fn_ty) => Type::Fn(FnType {
                params: fn_ty
                    .params
                    .iter()
                    .map(|param| param.unfold_inner(replacement))
                    .collect(),
                ret: Box::new(fn_ty.ret.unfold_inner(replacement)),
            }),
            Type::Never => Type::Never,
            Type::Var(id) => {
                if let Some((target_id, replacement_ty)) = replacement {
                    if *id == target_id {
                        return replacement_ty.clone();
                    }
                }
                Type::Var(*id)
            }
            Type::Record(record) => {
                Type::Record(record.map_types(|ty| ty.unfold_inner(replacement)))
            }
            Type::IsoRec(id, body) => {
                let rec = Type::IsoRec(*id, body.clone());
                body.unfold_inner(Some((*id, &rec)))
            }
        }
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Bool => write!(f, "Bool"),
            Type::Str => write!(f, "Str"),
            Type::Optional(ty) => write!(f, "{ty}?"),
            Type::List(ty) => write!(f, "[{ty}]"),
            Type::Fn(fn_ty) => write!(f, "{fn_ty}"),
            Type::Record(record) => write!(f, "{record}"),
            Type::IsoRec(id, ty) => write!(f, "IsoRec({id}, {ty})"),
            Type::Var(id) => write!(f, "Var({id})"),
            Type::Never => write!(f, "Never"),
        }
    }
}

impl std::fmt::Display for FnType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fn(")?;

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
