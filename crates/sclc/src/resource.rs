use crate::Record;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ResourceId {
    pub ty: String,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resource {
    pub inputs: Record,
    pub outputs: Record,
    pub dependencies: Vec<ResourceId>,
}
