use scylla::errors::{
    ExecutionError, FirstRowError, IntoRowsResultError, MaybeFirstRowError, NewSessionError,
    NextRowError, PagerExecutionError, PrepareError, RowsError, SingleRowError, TypeCheckError,
};
use thiserror::Error;

use crate::category::InvalidCategory;

/// Error returned when establishing a connection or initializing the SDB
/// schema.
#[derive(Error, Debug)]
pub enum ConnectError {
    #[error("failed to create session: {0}")]
    Scylla(#[from] NewSessionError),

    #[error("failed to prepare statement: {0}")]
    Prepare(#[from] PrepareError),

    #[error("failed to create tables: {0}")]
    CreateTables(#[from] ExecutionError),
}

/// Error returned by SDB read or write operations.
#[derive(Error, Debug)]
pub enum SdbError {
    #[error("failed to execute statement: {0}")]
    Execute(#[from] ExecutionError),

    #[error("failed to execute paged query: {0}")]
    Pager(#[from] PagerExecutionError),

    #[error("failed to convert query result into rows: {0}")]
    IntoRows(#[from] IntoRowsResultError),

    #[error("failed to type-check row: {0}")]
    TypeCheck(#[from] TypeCheckError),

    #[error("failed to load row: {0}")]
    NextRow(#[from] NextRowError),

    #[error("failed to read single row: {0}")]
    SingleRow(#[from] SingleRowError),

    #[error("failed to read first row: {0}")]
    FirstRow(#[from] FirstRowError),

    #[error("failed to read first row: {0}")]
    MaybeFirstRow(#[from] MaybeFirstRowError),

    #[error("failed to read rows: {0}")]
    Rows(#[from] RowsError),

    #[error("invalid category in database row: {0}")]
    InvalidCategory(#[from] InvalidCategory),

    #[error("invalid incident id in database row: {0}")]
    InvalidIncidentId(#[from] uuid::Error),
}
