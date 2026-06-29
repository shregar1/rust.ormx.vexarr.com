use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("builder error: {0}")]
    Builder(String),

    #[error("migration error: {0}")]
    Migration(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
