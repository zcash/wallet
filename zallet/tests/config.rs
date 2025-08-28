#![allow(clippy::bool_assert_comparison)]

use std::{env, fs, path::PathBuf, sync::Mutex};
use tempfile::{Builder, TempDir};

use zallet::config::ZalletConfig;
use zcash_protocol::consensus::NetworkType;

// Global mutex to ensure tests run sequentially to avoid env var races
static TEST_MUTEX: Mutex<()> = Mutex::new(());

/// Helper to isolate and manage ZALLET_* environment variables in tests.
struct EnvGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    original_vars: Vec<(String, String)>,
}

impl EnvGuard {
    /// Acquire the global lock and clear all ZALLET_* env vars, saving originals.
    fn new() -> Self {
        let guard = TEST_MUTEX.lock().unwrap_or_else(|poisoned| {
            // Handle poisoned mutex from previous test panics
            poisoned.into_inner()
        });

        let original_vars: Vec<(String, String)> = env::vars()
            .filter(|(key, _)| key.starts_with("ZALLET_"))
            .collect();

        for (key, _) in &original_vars {
            // SAFETY: We ensure single-threaded operation with `TEST_MUTEX`.
            unsafe {
                env::remove_var(key);
            }
        }

        Self {
            _guard: guard,
            original_vars,
        }
    }

    /// Set a ZALLET_* environment variable for this test.
    fn set_var(&mut self, key: &str, value: &str) {
        // SAFETY: We hold a lock on `TEST_MUTEX` to ensure single-threaded operation
        // relative to other tests, and we take `&mut self` to ensure this method isn't
        // called in parallel within a single test.
        unsafe {
            env::set_var(key, value);
        }
    }

    /// Create a temporary directory for test files.
    fn temp_dir(&self) -> TempDir {
        Builder::new()
            .prefix("zallet_config_test_")
            .tempdir()
            .expect("create temp dir")
    }

    /// Create a file with the given content in the provided directory.
    fn create_file(&self, dir: &TempDir, filename: &str, content: &str) -> PathBuf {
        let file_path = dir.path().join(filename);
        fs::write(&file_path, content).expect("write test file");
        file_path
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Clear any ZALLET_* set during the test
        let current_vars: Vec<String> = env::vars()
            .filter(|(key, _)| key.starts_with("ZALLET_"))
            .map(|(key, _)| key)
            .collect();
        for key in current_vars {
            // SAFETY: The `TEST_MUTEX` lock is not dropped until the end of this method,
            // and we have `&mut self` so we know this is the only drop method running.
            unsafe {
                env::remove_var(&key);
            }
        }

        // Restore originals
        for (key, value) in &self.original_vars {
            unsafe {
                env::set_var(key, value);
            }
        }
    }
}

// --- Test default configuration loading ---

#[test]
fn test_config_load_defaults() {
    let env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let empty_toml_path = env.create_file(&temp_dir, "empty.toml", "");
    let config = ZalletConfig::load(Some(&empty_toml_path)).expect("Should load default config");
    let default_config = ZalletConfig::default();

    assert_eq!(config.consensus.network, default_config.consensus.network);
    assert_eq!(config.rpc.bind, default_config.rpc.bind);
    assert_eq!(
        config.builder.spend_zeroconf_change(),
        default_config.builder.spend_zeroconf_change()
    );
}

#[test]
fn test_config_load_no_file() {
    let _env = EnvGuard::new();

    // Loading with no file should work (uses defaults)
    let config = ZalletConfig::load(None).expect("Should load default config");
    let default_config = ZalletConfig::default();

    assert_eq!(config.consensus.network, default_config.consensus.network);
    assert_eq!(config.rpc.bind, default_config.rpc.bind);
}

// --- Test TOML file loading ---

#[test]
fn test_deserialize_full_valid_config() {
    let env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let toml_content = r#"
[consensus]
network = "test"

[builder]
spend_zeroconf_change = false
trusted_confirmations = 5
tx_expiry_delta = 40
untrusted_confirmations = 10

[database]
wallet = "custom_wallet.db"

[external]
broadcast = false

[features]
as_of_version = "0.1.0"

[indexer]
validator_address = "127.0.0.1:8233"
validator_cookie_auth = true
validator_user = "test_user"
validator_password = "test_pass"
db_path = "custom_indexer_db"

[keystore]
encryption_identity = "test_identity.age"
require_backup = false

[note_management]
min_note_value = 10000
target_note_count = 5

[rpc]
bind = ["127.0.0.1:28232"]
timeout = 30
"#;

    let toml_path = env.create_file(&temp_dir, "test.toml", toml_content);
    let config = ZalletConfig::load(Some(&toml_path)).expect("Should load config from TOML");

    // Verify parsed values
    assert_eq!(config.consensus.network, NetworkType::Test);
    assert_eq!(config.builder.spend_zeroconf_change(), false);
    assert_eq!(config.builder.trusted_confirmations(), 5);
    assert_eq!(config.builder.tx_expiry_delta(), 40);
    assert_eq!(config.builder.untrusted_confirmations(), 10);
    assert_eq!(
        config.database.wallet,
        Some(PathBuf::from("custom_wallet.db"))
    );
    assert_eq!(config.external.broadcast(), false);
    assert_eq!(config.features.as_of_version, "0.1.0");
    assert_eq!(
        config.indexer.validator_address,
        Some("127.0.0.1:8233".to_string())
    );
    assert_eq!(config.indexer.validator_cookie_auth, Some(true));
    assert_eq!(config.indexer.validator_user, Some("test_user".to_string()));
    assert_eq!(
        config.indexer.validator_password,
        Some("test_pass".to_string())
    );
    assert_eq!(
        config.indexer.db_path,
        Some(PathBuf::from("custom_indexer_db"))
    );
    assert_eq!(
        config.keystore.encryption_identity,
        Some(PathBuf::from("test_identity.age"))
    );
    assert_eq!(config.keystore.require_backup(), false);
    assert_eq!(
        config.note_management.min_note_value(),
        10000u64.try_into().unwrap()
    );
    assert_eq!(config.note_management.target_note_count().get(), 5);
    assert_eq!(config.rpc.bind, vec!["127.0.0.1:28232".parse().unwrap()]);
    assert_eq!(config.rpc.timeout().as_secs(), 30);
}

// --- Test environment variable overrides ---

#[test]
fn test_env_override_consensus_network() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    // Create TOML with Mainnet
    let toml_content = r#"
[consensus]
network = "main"
"#;
    let toml_path = env.create_file(&temp_dir, "test.toml", toml_content);

    // Override with environment variable
    env.set_var("ZALLET_CONSENSUS__NETWORK", "test");

    let config = ZalletConfig::load(Some(&toml_path)).expect("Should load config");
    assert_eq!(config.consensus.network, NetworkType::Test);
}

#[test]
fn test_env_override_rpc_settings() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let toml_content = r#"
[rpc]
bind = ["127.0.0.1:28232"]
timeout = 60
"#;
    let toml_path = env.create_file(&temp_dir, "test.toml", toml_content);

    // Override with environment variables (only timeout; leave bind from TOML)
    env.set_var("ZALLET_RPC__TIMEOUT", "30");

    let config = ZalletConfig::load(Some(&toml_path)).expect("Should load config");
    assert_eq!(config.rpc.bind, vec!["127.0.0.1:28232".parse().unwrap()]);
    assert_eq!(config.rpc.timeout().as_secs(), 30);
}

#[test]
fn test_env_override_builder_settings() {
    let mut env = EnvGuard::new();

    // Test with no TOML file, only env vars
    env.set_var("ZALLET_BUILDER__SPEND_ZEROCONF_CHANGE", "false");
    env.set_var("ZALLET_BUILDER__TRUSTED_CONFIRMATIONS", "3");
    env.set_var("ZALLET_BUILDER__TX_EXPIRY_DELTA", "25");

    let config = ZalletConfig::load(None).expect("Should load config");
    assert_eq!(config.builder.spend_zeroconf_change(), false);
    assert_eq!(config.builder.trusted_confirmations(), 3);
    assert_eq!(config.builder.tx_expiry_delta(), 25);
}

#[test]
fn test_env_override_indexer_settings() {
    let mut env = EnvGuard::new();

    env.set_var("ZALLET_INDEXER__VALIDATOR_ADDRESS", "192.168.1.100:8233");
    env.set_var("ZALLET_INDEXER__VALIDATOR_COOKIE_AUTH", "true");
    env.set_var("ZALLET_INDEXER__VALIDATOR_USER", "env_user");
    env.set_var("ZALLET_INDEXER__DB_PATH", "/custom/indexer/path");

    let config = ZalletConfig::load(None).expect("Should load config");
    assert_eq!(
        config.indexer.validator_address,
        Some("192.168.1.100:8233".to_string())
    );
    assert_eq!(config.indexer.validator_cookie_auth, Some(true));
    assert_eq!(config.indexer.validator_user, Some("env_user".to_string()));
    assert_eq!(
        config.indexer.db_path,
        Some(PathBuf::from("/custom/indexer/path"))
    );
}

// --- Test precedence (environment variables should override TOML) ---

#[test]
fn test_env_precedence_over_toml() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    // TOML says one thing
    let toml_content = r#"
[consensus]
network = "main"

[builder]
spend_zeroconf_change = true
trusted_confirmations = 1

[rpc]
timeout = 60
"#;
    let toml_path = env.create_file(&temp_dir, "test.toml", toml_content);

    // Environment variables say another
    env.set_var("ZALLET_CONSENSUS__NETWORK", "regtest");
    env.set_var("ZALLET_BUILDER__SPEND_ZEROCONF_CHANGE", "false");
    env.set_var("ZALLET_BUILDER__TRUSTED_CONFIRMATIONS", "5");
    env.set_var("ZALLET_RPC__TIMEOUT", "30");

    let config = ZalletConfig::load(Some(&toml_path)).expect("Should load config");

    // Environment variables should win
    assert_eq!(config.consensus.network, NetworkType::Regtest);
    assert_eq!(config.builder.spend_zeroconf_change(), false);
    assert_eq!(config.builder.trusted_confirmations(), 5);
    assert_eq!(config.rpc.timeout().as_secs(), 30);
}

// --- Test error cases ---

#[test]
fn test_invalid_toml_file() {
    let env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let invalid_toml = "invalid toml content [[[";
    let toml_path = env.create_file(&temp_dir, "invalid.toml", invalid_toml);

    let result = ZalletConfig::load(Some(&toml_path));
    assert!(result.is_err());

    let error = result.unwrap_err();
    // config-rs provides structured error information for parse failures
    assert!(matches!(error, config::ConfigError::FileParse { .. }));
}

#[test]
fn test_missing_required_file() {
    let _env = EnvGuard::new();
    let non_existent_path = PathBuf::from("/non/existent/path/config.toml");

    let result = ZalletConfig::load(Some(&non_existent_path));
    assert!(result.is_err());
}

#[test]
fn test_invalid_env_var_value() {
    let mut env = EnvGuard::new();

    // Set an invalid network value
    env.set_var("ZALLET_CONSENSUS__NETWORK", "InvalidNetwork");

    let result = ZalletConfig::load(None);
    assert!(result.is_err());
}

#[test]
fn test_invalid_env_var_format() {
    let mut env = EnvGuard::new();

    // Set invalid socket address format
    env.set_var(
        "ZALLET_RPC__BIND",
        "invalid_socket_address,127.0.0.1:invalid_port",
    );

    let result = ZalletConfig::load(None);
    assert!(result.is_err());
}

// --- Test complex scenarios ---

#[test]
fn test_partial_env_override() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let toml_content = r#"
[consensus]
network = "main"

[builder]
spend_zeroconf_change = true
trusted_confirmations = 1
tx_expiry_delta = 40

[rpc]
bind = ["127.0.0.1:28232"]
timeout = 60
"#;
    let toml_path = env.create_file(&temp_dir, "test.toml", toml_content);

    // Only override some values
    env.set_var("ZALLET_CONSENSUS__NETWORK", "test");
    env.set_var("ZALLET_RPC__TIMEOUT", "30");

    let config = ZalletConfig::load(Some(&toml_path)).expect("Should load config");

    // Overridden values
    assert_eq!(config.consensus.network, NetworkType::Test);
    assert_eq!(config.rpc.timeout().as_secs(), 30);

    // Non-overridden values should remain from TOML
    assert_eq!(config.builder.spend_zeroconf_change(), true);
    assert_eq!(config.builder.trusted_confirmations(), 1);
    assert_eq!(config.builder.tx_expiry_delta(), 40);
    assert_eq!(config.rpc.bind, vec!["127.0.0.1:28232".parse().unwrap()]);
}

#[test]
fn test_rpc_bind_multiple_addresses() {
    let mut env = EnvGuard::new();

    // Test comma-separated list of bind addresses
    env.set_var(
        "ZALLET_RPC__BIND",
        "127.0.0.1:28232,127.0.0.1:28233,0.0.0.0:28234",
    );

    let config = ZalletConfig::load(None).expect("Should load config");

    assert_eq!(config.rpc.bind.len(), 3);
    assert_eq!(config.rpc.bind[0], "127.0.0.1:28232".parse().unwrap());
    assert_eq!(config.rpc.bind[1], "127.0.0.1:28233".parse().unwrap());
    assert_eq!(config.rpc.bind[2], "0.0.0.0:28234".parse().unwrap());
}

#[test]
fn test_boolean_env_parsing() {
    let mut env = EnvGuard::new();

    // Test various boolean representations
    env.set_var("ZALLET_BUILDER__SPEND_ZEROCONF_CHANGE", "false");
    env.set_var("ZALLET_EXTERNAL__BROADCAST", "true");
    env.set_var("ZALLET_INDEXER__VALIDATOR_COOKIE_AUTH", "1"); // Should parse as true
    env.set_var("ZALLET_KEYSTORE__REQUIRE_BACKUP", "0"); // Should parse as false

    let config = ZalletConfig::load(None).expect("Should load config");
    assert_eq!(config.builder.spend_zeroconf_change(), false);
    assert_eq!(config.external.broadcast(), true);
    assert_eq!(config.indexer.validator_cookie_auth, Some(true));
    assert_eq!(config.keystore.require_backup(), false);
}

#[test]
fn test_numeric_env_parsing() {
    let mut env = EnvGuard::new();

    env.set_var("ZALLET_BUILDER__TRUSTED_CONFIRMATIONS", "42");
    env.set_var("ZALLET_BUILDER__TX_EXPIRY_DELTA", "100");
    env.set_var("ZALLET_RPC__TIMEOUT", "120");
    env.set_var("ZALLET_NOTE_MANAGEMENT__MIN_NOTE_VALUE", "50000");

    let config = ZalletConfig::load(None).expect("Should load config");
    assert_eq!(config.builder.trusted_confirmations(), 42);
    assert_eq!(config.builder.tx_expiry_delta(), 100);
    assert_eq!(config.rpc.timeout().as_secs(), 120);
    assert_eq!(
        config.note_management.min_note_value(),
        50000u64.try_into().unwrap()
    );
}

// --- Sensitive env deny-list behavior ---

#[test]
fn test_env_unknown_non_sensitive_key_errors() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let test_config_path = env.create_file(&temp_dir, "test_config.toml", "");
    // Unknown non-sensitive key should cause an error due to deny_unknown_fields
    env.set_var("ZALLET_FOO", "bar");
    let result = ZalletConfig::load(Some(&test_config_path));
    assert!(
        result.is_err(),
        "Unknown non-sensitive env key should error (deny_unknown_fields)"
    );
}

#[test]
fn test_env_unknown_sensitive_key_errors() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let test_config_path = env.create_file(&temp_dir, "test_config.toml", "");
    // Unknown sensitive-suffix key should cause an error
    env.set_var("ZALLET_SOME__SECRET", "topsecret");
    let result = ZalletConfig::load(Some(&test_config_path));
    assert!(
        result.is_err(),
        "Unknown sensitive-suffix env keys should cause configuration loading to fail"
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("sensitive key 'SECRET'"),
        "Error message should mention the sensitive key"
    );
}

#[test]
fn test_env_validator_password_errors() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let test_config_path = env.create_file(&temp_dir, "test_config.toml", "");
    // validator_password should cause configuration loading to fail
    env.set_var("ZALLET_INDEXER__VALIDATOR_PASSWORD", "topsecret");
    let result = ZalletConfig::load(Some(&test_config_path));

    assert!(
        result.is_err(),
        "Sensitive env keys should cause configuration loading to fail"
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("sensitive key 'VALIDATOR_PASSWORD'"),
        "Error message should mention the sensitive key"
    );
}

#[test]
fn test_env_non_sensitive_keys_still_work() {
    let mut env = EnvGuard::new();
    let temp_dir = env.temp_dir();

    let test_config_path = env.create_file(&temp_dir, "test_config.toml", "");
    // Non-sensitive keys should still work
    env.set_var("ZALLET_INDEXER__VALIDATOR_ADDRESS", "127.0.0.1:8233");
    env.set_var("ZALLET_INDEXER__VALIDATOR_USER", "testuser");
    env.set_var("ZALLET_INDEXER__VALIDATOR_COOKIE_PATH", "/path/to/cookie");

    let config = ZalletConfig::load(Some(&test_config_path)).expect("Should load config");

    // Non-sensitive fields should be set from env
    assert_eq!(
        config.indexer.validator_address,
        Some("127.0.0.1:8233".to_string())
    );
    assert_eq!(config.indexer.validator_user, Some("testuser".to_string()));
    assert_eq!(
        config.indexer.validator_cookie_path,
        Some("/path/to/cookie".to_string())
    );
    // Sensitive field should remain default (None)
    assert_eq!(config.indexer.validator_password, None);
}
