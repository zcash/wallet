//! Zallet Config

use serde::{Deserialize, Serialize};

/// Zallet Configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZalletConfig {}

/// Default configuration settings.
impl Default for ZalletConfig {
    fn default() -> Self {
        Self {}
    }
}
