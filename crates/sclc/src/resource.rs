use crate::Record;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceId {
    pub ty: String,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resource {
    pub inputs: Record,
    pub outputs: Record,
}
