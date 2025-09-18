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

use abscissa_core::testing::prelude::*;
use once_cell::sync::Lazy;
use tempfile::tempdir;

#[cfg(zallet_build = "wallet")]
use {
    abscissa_core::fs::File,
    age::secrecy::ExposeSecret,
    std::io::{BufRead, Write},
};

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
    let datadir = tempdir().unwrap();
    let mut runner = RUNNER.clone();
    let cmd = runner
        .arg("--datadir")
        .arg(datadir.path())
        .arg("start")
        .run();
    cmd.wait().unwrap().expect_code(1);
}

/// Example of a test which matches a regular expression
#[test]
fn version_no_args() {
    let mut runner = RUNNER.clone();
    let mut cmd = runner.arg("--version").capture_stdout().run();
    cmd.stdout().expect_regex(r"\A\w+ [\d\.\-a-z]+\z");
}

#[cfg(zallet_build = "wallet")]
#[test]
fn setup_new_wallet() {
    let datadir = tempdir().unwrap();
    let config_file = datadir.path().join("zallet.toml");
    let identity_file = datadir.path().join("encryption-identity.txt");

    {
        let mut f = File::create(&config_file).unwrap();
        writeln!(f, "[builder]").unwrap();
        writeln!(f, "[builder.limits]").unwrap();
        writeln!(f, "[consensus]").unwrap();
        writeln!(f, "network = \"test\"").unwrap();
        writeln!(f, "[database]").unwrap();
        writeln!(f, "[external]").unwrap();
        writeln!(f, "[features]").unwrap();
        writeln!(f, "as_of_version = \"{}\"", env!("CARGO_PKG_VERSION")).unwrap();
        writeln!(f, "[features.deprecated]").unwrap();
        writeln!(f, "[features.experimental]").unwrap();
        writeln!(f, "[indexer]").unwrap();
        writeln!(f, "validator_address = \"127.0.0.1:65536\"").unwrap();
        writeln!(f, "[keystore]").unwrap();
        writeln!(f, "[note_management]").unwrap();
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
            .arg("--datadir")
            .arg(datadir.path())
            .arg("init-wallet-encryption")
            .capture_stderr()
            .run();
        let stderr = cmd.stderr();
        wait_until_running(stderr);
        stderr.expect_regex(".*Creating empty database.*");
        cmd.wait().unwrap().expect_code(0);
    }

    {
        let mut runner = RUNNER.clone();
        let mut cmd = runner
            .arg("--datadir")
            .arg(datadir.path())
            .arg("generate-mnemonic")
            .capture_stderr()
            .run();
        let stderr = cmd.stderr();
        wait_until_running(stderr);
        stderr.expect_regex(".*Applying latest database migrations.*");
        cmd.wait().unwrap().expect_code(0);
    }

    {
        let mut runner = RUNNER.clone();
        let cmd = runner
            .arg("--datadir")
            .arg(datadir.path())
            .arg("start")
            .run();
        // We added invalid data to the config in order to ensure that `start` fails.
        cmd.wait().unwrap().expect_code(1);
    }
}

#[cfg(zallet_build = "wallet")]
fn wait_until_running(stderr: &mut Stderr) {
    let mut buf = String::new();
    while !buf.contains("Running") {
        stderr.read_line(&mut buf).unwrap();
    }
}
