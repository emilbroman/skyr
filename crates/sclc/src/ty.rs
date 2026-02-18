use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    Int,
    Record(RecordType),
    IsoRec(usize, Box<Type>),
    Var(usize),
    Never,
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
            Type::Record(record) => write!(f, "{record}"),
            Type::IsoRec(id, ty) => write!(f, "IsoRec({id}, {ty})"),
            Type::Var(id) => write!(f, "Var({id})"),
            Type::Never => write!(f, "Never"),
        }
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
