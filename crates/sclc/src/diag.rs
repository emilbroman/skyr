use std::error::Error;

use crate::{ModuleId, Span};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagLevel {
    Error,
    Warning,
}

pub trait Diag: Error + Send + Sync {
    fn locate(&self) -> (ModuleId, Span);

    fn level(&self) -> DiagLevel {
        DiagLevel::Error
    }
}

#[derive(Default)]
pub struct DiagList {
    diags: Vec<Box<dyn Diag>>,
}

impl std::fmt::Debug for DiagList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagList")
            .field("count", &self.diags.len())
            .finish()
    }
}

impl DiagList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diag: impl Diag + 'static) {
        self.diags.push(Box::new(diag));
    }

    pub fn iter(&self) -> impl Iterator<Item = &(dyn Diag + 'static)> {
        self.diags.iter().map(Box::as_ref)
    }

    pub fn is_empty(&self) -> bool {
        self.diags.is_empty()
    }

    pub fn has_errors(&self) -> bool {
        self.diags
            .iter()
            .any(|diag| diag.level() == DiagLevel::Error)
    }

    pub fn extend(&mut self, other: Self) {
        self.diags.extend(other.diags);
    }

    /// Remove duplicate diagnostics (same location and message).
    ///
    /// PEG backtracking does not roll back mutations to the shared `&mut
    /// DiagList`, so a diagnostic pushed inside a rule that ultimately fails
    /// can be re-pushed when the same input is re-parsed via a different
    /// grammar path. Call this after parsing to collapse those duplicates.
    pub fn dedup(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.diags.retain(|d| {
            let (module_id, span) = d.locate();
            seen.insert((module_id, span, d.to_string()))
        });
    }
}

#[derive(Default)]
pub struct Diagnosed<T> {
    value: T,
    diags: DiagList,
}

impl<T> std::fmt::Debug for Diagnosed<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Diagnosed")
            .field("value_type", &std::any::type_name::<T>())
            .field("diags", &self.diags)
            .finish()
    }
}

impl<T> Diagnosed<T> {
    pub fn new(value: T, diags: DiagList) -> Self {
        Self { value, diags }
    }

    pub fn diags(&self) -> &DiagList {
        &self.diags
    }

    pub fn diags_mut(&mut self) -> &mut DiagList {
        &mut self.diags
    }

    pub fn into_inner(self) -> T {
        self.value
    }

    pub fn unpack(self, into: &mut DiagList) -> T {
        let Diagnosed { value, diags } = self;
        into.extend(diags);
        value
    }
}

impl<T> AsRef<T> for Diagnosed<T> {
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl<T> AsMut<T> for Diagnosed<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T> std::ops::Deref for Diagnosed<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> std::ops::DerefMut for Diagnosed<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}
