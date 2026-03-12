use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::{Position, Span, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionCandidate {
    Var(String),
    Member(String),
}

#[derive(Debug)]
pub struct CursorInfo {
    pub ty: Option<Type>,
    pub declaration: Option<Span>,
    pub references: Vec<Span>,
    pub completion_candidates: Vec<CompletionCandidate>,
    /// Buffers (declaration_span → reference_spans) collected before the cursor's
    /// own declaration is known. Flushed into `references` by `set_declaration`.
    ref_tracking: BTreeMap<Span, Vec<Span>>,
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

    pub fn set_declaration(&self, span: Span) {
        let mut inner = self.inner.lock().unwrap();
        inner.declaration = Some(span);
        // Flush any buffered references for this declaration
        if let Some(refs) = inner.ref_tracking.remove(&span) {
            inner.references = refs;
        }
    }

    /// Record a reference to a declaration. If the cursor's declaration is already
    /// known and matches, the reference is added directly. Otherwise it is buffered
    /// until `set_declaration` identifies which declaration the cursor points to.
    pub fn track_reference(&self, declaration: Span, reference: Span) {
        let mut inner = self.inner.lock().unwrap();
        if inner.declaration == Some(declaration) {
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
