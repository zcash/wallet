//! Acceptance test: runs the application as a subprocess and asserts its
//! output for given argument combinations matches what is expected.
//!
//! For more information, see:
//! <https://docs.rs/abscissa_core/latest/abscissa_core/testing/index.html>

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    rust_2018_idioms,
    trivial_casts,
    unused_lifetimes,
    unused_qualifications
)]

use std::io::Write;

use abscissa_core::{fs::File, testing::prelude::*};
use age::secrecy::ExposeSecret;
use once_cell::sync::Lazy;
use tempfile::tempdir;

/// Executes your application binary via `cargo run`.
///
/// Storing this value as a [`Lazy`] static ensures that all instances of
/// the runner acquire a mutex when executing commands and inspecting
/// exit statuses, serializing what would otherwise be multithreaded
/// invocations as `cargo test` executes tests in parallel by default.
pub static RUNNER: Lazy<CmdRunner> = Lazy::new(CmdRunner::default);

/// Use `ZalletConfig::default()` value if no config or args
#[test]
fn start_no_args() {
    let mut runner = RUNNER.clone();
    let mut cmd = runner.arg("start").capture_stdout().run();
    cmd.stdout().expect_line("");
    cmd.wait().unwrap().expect_code(1);
}

/// Example of a test which matches a regular expression
#[test]
fn version_no_args() {
    let mut runner = RUNNER.clone();
    let mut cmd = runner.arg("--version").capture_stdout().run();
    cmd.stdout().expect_regex(r"\A\w+ [\d\.\-]+\z");
}

#[test]
fn setup_new_wallet() {
    let datadir = tempdir().unwrap();
    let config_file = datadir.path().join("zallet.toml");
    let identity_file = datadir.path().join("identity.txt");
    let wallet_db = datadir.path().join("data.sqlite");

    {
        let mut f = File::create(&config_file).unwrap();
        writeln!(f, "network = \"test\"").unwrap();
        writeln!(f, "wallet_db = \"{}\"", wallet_db.display()).unwrap();
        writeln!(f, "[builder]").unwrap();
        writeln!(f, "[indexer]").unwrap();
        writeln!(f, "[keystore]").unwrap();
        writeln!(f, "identity = \"{}\"", identity_file.display()).unwrap();
        writeln!(f, "[limits]").unwrap();
        writeln!(f, "[rpc]").unwrap();
        writeln!(f, "bind = []").unwrap();
    }

    {
        let mut f = File::create(identity_file).unwrap();
        writeln!(
            f,
            "{}",
            age::x25519::Identity::generate()
                .to_string()
                .expose_secret()
        )
        .unwrap();
    }

    {
        let mut runner = RUNNER.clone();
        let mut cmd = runner
            .arg("-c")
            .arg(&config_file)
            .arg("init-wallet-encryption")
            .capture_stdout()
            .run();
        cmd.stdout().expect_regex(".*Creating empty database.*");
        cmd.wait().unwrap().expect_code(0);
    }

    {
        let mut runner = RUNNER.clone();
        let mut cmd = runner
            .arg("-c")
            .arg(&config_file)
            .arg("generate-mnemonic")
            .capture_stdout()
            .run();
        cmd.stdout()
            .expect_regex(".*Applying latest database migrations.*");
        cmd.wait().unwrap().expect_code(0);
    }

    {
        let mut runner = RUNNER.clone();
        let cmd = runner.arg("-c").arg(&config_file).arg("start").run();
        // We omitted some config lines necessary for `start` in order to ensure
        // that it fails.
        cmd.wait().unwrap().expect_code(1);
    }
}
