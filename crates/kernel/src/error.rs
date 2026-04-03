use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Module error: {0}")]
    Module(String),
    
    #[error("AI error: {0}")]
    Ai(String),
    
    #[error("Tool error: {0}")]
    Tool(String),
    
    #[error("Security error: {0}")]
    Security(String),
    
    #[error("HTTP error: {0}")]
    Http(String),
    
    #[error("Unknown error: {0}")]
    Unknown(String),
}

pub type Result<T> = std::result::Result<T, Error>;
