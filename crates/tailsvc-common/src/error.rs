use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid hostname: {0}")]
    InvalidHostname(String),
    #[error("invalid backend URL: {0}")]
    InvalidBackend(String),
    #[error("label parse error: {0}")]
    LabelParse(String),
    #[error("ambiguous backend resolution: {0}")]
    AmbiguousBackend(String),
    #[error("authentication failed")]
    Auth,
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("{0}")]
    Other(String),
}
