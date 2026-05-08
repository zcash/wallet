use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use std::fs;
use tempfile::tempdir;
use zallet_parity_testkit::MockNode;

#[tokio::test]
async fn test_cli_run_success() -> Result<(), Box<dyn std::error::Error>> {
    let upstream = MockNode::spawn().await;
    let target = MockNode::spawn().await;

    // Mock responses
    upstream
        .mock_response("getblockchaininfo", json!(null), json!({"blocks": 100}))
        .await;
    target
        .mock_response("getblockchaininfo", json!(null), json!({"blocks": 100}))
        .await;

    let dir = tempdir()?;
    let manifest_path = dir.path().join("manifest.toml");
    let report_path = dir.path().join("report.json");

    fs::write(
        &manifest_path,
        r#"
[[methods]]
name = "getblockchaininfo"
"#,
    )?;

    let mut cmd = Command::cargo_bin("zallet-rpc-diff")?;
    cmd.arg("run")
        .arg("--upstream-url")
        .arg(upstream.url())
        .arg("--target-url")
        .arg(target.url())
        .arg("--manifest")
        .arg(&manifest_path)
        .arg("--output")
        .arg(&report_path);

    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Parity check complete!"))
        .stdout(predicate::str::contains("1 total | ✅ 1 match | ❌ 0 diff"));

    // Verify report files exist
    assert!(report_path.exists());
    let report_content = fs::read_to_string(&report_path)?;
    assert!(report_content.contains("\"matches\": 1"));

    let md_path = report_path.with_extension("md");
    assert!(md_path.exists());
    let md_content = fs::read_to_string(&md_path)?;
    assert!(md_content.contains("✅ Matches**: 1"));

    Ok(())
}
