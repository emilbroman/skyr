use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::asg::RawModuleId;
use crate::{Position, Span, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionMember {
    pub name: String,
    pub description: Option<String>,
    pub ty: Option<Type>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionCandidate {
    Var(String),
    Member(CompletionMember),
    /// An importable `.scl` module.
    Module(String),
    /// A directory that may contain further modules.
    ModuleDir(String),
    /// A file (with extension) for path completion.
    PathFile(String),
    /// A directory for path completion.
    PathDir(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CursorIdentifier {
    Let(String),
    Type(String),
}

#[derive(Debug)]
pub struct CursorInfo {
    pub ty: Option<Type>,
    pub identifier: Option<CursorIdentifier>,
    pub description: Option<String>,
    pub declaration: Option<(RawModuleId, Span)>,
    pub references: Vec<(RawModuleId, Span)>,
    pub completion_candidates: Vec<CompletionCandidate>,
    /// Buffers (declaration_key → reference_locations) collected before the
    /// cursor's own declaration is known. Flushed into `references` by
    /// `set_declaration`.
    ref_tracking: BTreeMap<(RawModuleId, Span), Vec<(RawModuleId, Span)>>,
}

#[derive(Clone, Debug)]
pub struct Cursor {
    pub position: Position,
    inner: Arc<Mutex<CursorInfo>>,
}

impl Cursor {
    pub fn new(position: Position) -> Self {
        Self {
            position,
            inner: Arc::new(Mutex::new(CursorInfo {
                ty: None,
                identifier: None,
                description: None,
                declaration: None,
                references: Vec::new(),
                completion_candidates: Vec::new(),
                ref_tracking: BTreeMap::new(),
            })),
        }
    }

    pub fn set_type(&self, ty: Type) {
        self.inner.lock().unwrap().ty = Some(ty);
    }

    /// Apply type substitutions to the stored type (if any).
    /// Used after SCC constraint solving to replace free type variables.
    pub fn substitute_type(&self, substitutions: &[(usize, Type)]) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ty) = &inner.ty {
            inner.ty = Some(ty.substitute(substitutions));
        }
    }

    pub fn set_identifier(&self, identifier: CursorIdentifier) {
        self.inner.lock().unwrap().identifier = Some(identifier);
    }

    pub fn set_description(&self, description: String) {
        self.inner.lock().unwrap().description = Some(description);
    }

    pub fn set_declaration(&self, module: RawModuleId, span: Span) {
        let mut inner = self.inner.lock().unwrap();
        let key = (module.clone(), span);
        inner.declaration = Some(key.clone());
        // Flush any buffered references for this declaration
        if let Some(refs) = inner.ref_tracking.remove(&key) {
            inner.references = refs;
        }
    }

    /// Record a reference to a declaration. If the cursor's declaration is already
    /// known and matches, the reference is added directly. Otherwise it is buffered
    /// until `set_declaration` identifies which declaration the cursor points to.
    pub fn track_reference(
        &self,
        declaration: (RawModuleId, Span),
        reference: (RawModuleId, Span),
    ) {
        let mut inner = self.inner.lock().unwrap();
        if inner.declaration.as_ref() == Some(&declaration) {
            inner.references.push(reference);
        } else {
            inner
                .ref_tracking
                .entry(declaration)
                .or_default()
                .push(reference);
        }
    }

    pub fn add_completion_candidate(&self, candidate: CompletionCandidate) {
        self.inner
            .lock()
            .unwrap()
            .completion_candidates
            .push(candidate);
    }

    pub fn info(&self) -> Arc<Mutex<CursorInfo>> {
        self.inner.clone()
    }
}
