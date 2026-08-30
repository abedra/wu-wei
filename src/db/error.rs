#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid uuid stored in database: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("invalid enum value stored in database: {0}")]
    InvalidEnumValue(String),
    #[error("no row found for {0}")]
    NotFound(String),
}

pub type DbResult<T> = Result<T, DbError>;
