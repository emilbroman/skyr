use std::collections::BTreeSet;

use crate::Record;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ResourceId {
    pub ty: String,
    pub id: String,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Marker {
    Volatile,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resource {
    pub inputs: Record,
    pub outputs: Record,
    pub dependencies: Vec<ResourceId>,
    pub markers: BTreeSet<Marker>,
}

impl Resource {
    pub fn is_volatile(&self) -> bool {
        self.markers.contains(&Marker::Volatile)
    }
}
