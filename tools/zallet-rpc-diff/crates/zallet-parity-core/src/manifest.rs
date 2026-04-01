use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::{Result, Error};

/// A manifest defining the suite of RPC methods to test.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub methods: Vec<MethodEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MethodEntry {
    pub name: String,
    pub params: Option<serde_json::Value>,
}

impl Manifest {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::Manifest(format!("Failed to read manifest: {}", e)))?;
        
        toml::from_str(&content)
            .map_err(|e| Error::Manifest(format!("Failed to parse manifest: {}", e)))
    }
}
