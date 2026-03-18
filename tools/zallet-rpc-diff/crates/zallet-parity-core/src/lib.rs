use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Transport error: {0}")]
    Transport(String),

    #[error("RPC error (code {code}): {message}")]
    Rpc { code: i32, message: String },

    #[error("Manifest error: {0}")]
    Manifest(String),

    #[error("Normalization error: {0}")]
    Normalization(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod manifest;
pub mod report;
pub mod types;
