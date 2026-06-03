use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A manifest defining the suite of RPC methods to test.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub methods: Vec<MethodEntry>,
}

/// A single method entry in the parity manifest.
///
/// # Example TOML
/// ```toml
/// [[methods]]
/// name = "getblockchaininfo"
/// ignore_paths = ["/blocks", "/verificationprogress"]
/// ```
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MethodEntry {
    pub name: String,
    pub params: Option<serde_json::Value>,
    /// JSON Pointer paths (RFC 6901) to remove from both responses
    /// before comparison. Useful for volatile or intentionally-divergent fields.
    #[serde(default)]
    pub ignore_paths: Vec<String>,
}

impl Manifest {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::Manifest(format!("Failed to read manifest: {}", e)))?;

        toml::from_str(&content)
            .map_err(|e| Error::Manifest(format!("Failed to parse manifest: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_load_manifest_with_ignore_paths() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"
[[methods]]
name = "getblockchaininfo"
ignore_paths = ["/blocks", "/verificationprogress"]

[[methods]]
name = "getwalletinfo"
"#
        )
        .unwrap();

        let manifest = Manifest::load(file.path()).unwrap();
        assert_eq!(manifest.methods.len(), 2);

        let m0 = &manifest.methods[0];
        assert_eq!(m0.name, "getblockchaininfo");
        assert_eq!(m0.ignore_paths, vec!["/blocks", "/verificationprogress"]);

        let m1 = &manifest.methods[1];
        assert_eq!(m1.name, "getwalletinfo");
        assert!(m1.ignore_paths.is_empty()); // defaults to empty vec
    }

    #[test]
    fn test_load_manifest_no_ignore_paths_defaults_empty() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[[methods]]\nname = \"getinfo\"").unwrap();

        let manifest = Manifest::load(file.path()).unwrap();
        assert!(manifest.methods[0].ignore_paths.is_empty());
    }
}
