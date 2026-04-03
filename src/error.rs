use std::io;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid path")]
    InvalidPath,

    #[error("Not found")]
    NotFound,

    #[error("Permission denied")]
    PermissionDenied,
}

impl Error {
    pub fn status_code(&self) -> http::StatusCode {
        match self {
            Error::NotFound => http::StatusCode::NOT_FOUND,
            Error::PermissionDenied => http::StatusCode::FORBIDDEN,
            _ => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
