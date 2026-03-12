use std::sync::{Arc, Mutex};

use crate::{Position, Type};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionCandidate {
    Var(String),
    Member(String),
}

#[derive(Debug)]
pub struct CursorInfo {
    pub ty: Option<Type>,
    pub completion_candidates: Vec<CompletionCandidate>,
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
                completion_candidates: Vec::new(),
            })),
        }
    }

    pub fn set_type(&self, ty: Type) {
        self.inner.lock().unwrap().ty = Some(ty);
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
