use ids::ParseIdError;
use scylla::errors::{
    ExecutionError, FirstRowError, IntoRowsResultError, NewSessionError, NextRowError,
    PagerExecutionError, PrepareError, TypeCheckError,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create session: {0}")]
    Scylla(#[from] NewSessionError),

    #[error("failed to prepare statement: {0}")]
    Prepare(#[from] PrepareError),

    #[error("failed to create tables: {0}")]
    CreateTables(#[from] ExecutionError),
}

#[derive(Error, Debug)]
pub enum ReserveError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("failed to load result: {0}")]
    IntoRows(#[from] IntoRowsResultError),

    #[error("failed to parse row: {0}")]
    FirstRow(#[from] FirstRowError),

    #[error("name already taken")]
    NameTaken,
}

#[derive(Error, Debug)]
pub enum LookupError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("failed to execute statement: {0}")]
    Pager(#[from] PagerExecutionError),

    #[error("failed to parse row: {0}")]
    TypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    NextRow(#[from] NextRowError),

    #[error("invalid region in database: {0}")]
    InvalidRegion(#[from] ParseIdError),
}
