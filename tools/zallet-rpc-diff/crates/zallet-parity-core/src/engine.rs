use tokio::task::JoinSet;
use serde_json::Value;
use crate::client::RpcClient;
use crate::differ::{diff_values, DiffEntry};
use crate::expected_diffs::ExpectedDiffs;
use crate::normalizer::{normalize, parse_ignore_paths};
use crate::Error;

/// JSON-RPC "method not found" error code per spec.
const METHOD_NOT_FOUND_CODE: i32 = -32601;

/// Returns `true` if the given error represents a "method not found" response.
fn is_method_not_found(err: &Error) -> bool {
    match err {
        Error::JsonRpc(e) => {
            e.to_string().contains(&METHOD_NOT_FOUND_CODE.to_string())
                || e.to_string().to_lowercase().contains("method not found")
        }
        _ => false,
    }
}

/// The result of a single parity check.
#[derive(Debug, Clone)]
pub enum ParityResult {
    /// Both endpoints returned identical data (after normalization).
    Match,
    /// Both endpoints returned data, but the normalized values differ —
    /// and this difference was NOT anticipated by the expected-diffs file.
    Diff {
        /// Structured list of leaf-level differences (with JSON Pointer paths).
        diff_entries: Vec<DiffEntry>,
    },
    /// The diff was found in the expected-diffs file — it is a known,
    /// intentional divergence. Visible in the report but not a blocker.
    ExpectedDiff {
        diff_entries: Vec<DiffEntry>,
        reason: String,
    },
    /// One or both endpoints returned -32601 "method not found".
    Missing {
        method: String,
    },
    /// A transport failure or non-missing RPC error occurred.
    Error(String),
}

/// The engine responsible for executing the parity suite.
pub struct ParityEngine {
    upstream: RpcClient,
    target: RpcClient,
}

impl ParityEngine {
    pub fn new(upstream: RpcClient, target: RpcClient) -> Self {
        Self { upstream, target }
    }

    /// Runs the parity checks for all methods defined in the manifest.
    ///
    /// Each method is executed concurrently via `tokio::task::JoinSet`.
    /// The normalization pipeline (key-sort + ignore-paths) is applied
    /// before comparison. If a diff is found and it matches an entry in
    /// `expected_diffs`, it is classified as `ExpectedDiff` instead of `Diff`.
    pub async fn run_all(
        &self,
        methods: Vec<crate::manifest::MethodEntry>,
        expected_diffs: &ExpectedDiffs,
    ) -> Vec<(String, ParityResult)> {
        let mut set = JoinSet::new();
        let mut results = Vec::new();

        // Clone expected entries so they can be moved into spawned tasks
        let expected_entries: Vec<_> = expected_diffs.expected.clone();

        for entry in methods {
            let upstream = self.upstream.clone();
            let target = self.target.clone();
            let method_name = entry.name.clone();
            let params = entry.params.unwrap_or(Value::Null);
            let raw_ignore_paths = entry.ignore_paths.clone();
            let expected = expected_entries.clone();

            set.spawn(async move {
                // Parse ignore paths — log and skip on invalid pointers
                let ignore_paths = match parse_ignore_paths(&raw_ignore_paths) {
                    Ok(paths) => paths,
                    Err(e) => {
                        tracing::warn!("Invalid ignore path for '{}': {}", method_name, e);
                        vec![]
                    }
                };

                let res_u = upstream.call(&method_name, params.clone()).await;
                let res_t = target.call(&method_name, params).await;

                let parity = match (res_u, res_t) {
                    (Ok(u), Ok(t)) => {
                        // Apply normalization pipeline before comparison
                        let u_norm = normalize(u, &ignore_paths);
                        let t_norm = normalize(t, &ignore_paths);

                        let entries = diff_values(&u_norm, &t_norm);

                        if entries.is_empty() {
                            ParityResult::Match
                        } else {
                            // Check if this diff is known/expected
                            let actual_paths: Vec<String> =
                                entries.iter().map(|e| e.path.clone()).collect();

                            let expected_entry = expected.iter().find(|ee| {
                                if ee.method != method_name {
                                    return false;
                                }
                                if ee.diff_paths.is_empty() {
                                    return true;
                                }
                                actual_paths.iter().all(|p| {
                                    ee.diff_paths.iter().any(|ep| p.starts_with(ep.as_str()))
                                })
                            });

                            if let Some(ee) = expected_entry {
                                ParityResult::ExpectedDiff {
                                    diff_entries: entries,
                                    reason: ee.reason.clone(),
                                }
                            } else {
                                ParityResult::Diff { diff_entries: entries }
                            }
                        }
                    }
                    // Both sides: method not found
                    (Err(ref e_u), Err(ref e_t))
                        if is_method_not_found(e_u) && is_method_not_found(e_t) =>
                    {
                        ParityResult::Missing { method: method_name.clone() }
                    }
                    // One side: method not found
                    (Err(ref e), _) if is_method_not_found(e) => {
                        ParityResult::Missing { method: method_name.clone() }
                    }
                    (_, Err(ref e)) if is_method_not_found(e) => {
                        ParityResult::Missing { method: method_name.clone() }
                    }
                    // Other upstream error
                    (Err(e), _) => ParityResult::Error(format!("Upstream error: {}", e)),
                    // Other target error
                    (_, Err(e)) => ParityResult::Error(format!("Target error: {}", e)),
                };

                (method_name, parity)
            });
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(tagged_res) => results.push(tagged_res),
                Err(e) => tracing::error!("Task failed: {}", e),
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zallet_parity_testkit::MockNode;
    use crate::expected_diffs::{ExpectedDiffEntry, ExpectedDiffs};
    use crate::manifest::MethodEntry;
    use serde_json::json;

    fn entry(name: &str) -> MethodEntry {
        MethodEntry { name: name.to_string(), params: None, ignore_paths: vec![] }
    }

    fn entry_with_ignore(name: &str, paths: Vec<&str>) -> MethodEntry {
        MethodEntry {
            name: name.to_string(),
            params: None,
            ignore_paths: paths.into_iter().map(String::from).collect(),
        }
    }

    fn no_expected() -> ExpectedDiffs { ExpectedDiffs::none() }

    // ── MATCH ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_match() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_match";
        let resp = json!({"blocks": 100, "chain": "main"});
        u.mock_response(method, json!(null), resp.clone()).await;
        t.mock_response(method, json!(null), resp).await;
        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &no_expected()).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match));
    }

    // ── MATCH via normalization ───────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_ordering_only_diff_is_match_after_normalization() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_ordering";
        u.mock_response(method, json!(null), json!({"z": 1, "a": 2, "m": 3})).await;
        t.mock_response(method, json!(null), json!({"a": 2, "m": 3, "z": 1})).await;
        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &no_expected()).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match),
            "ordering-only diff should be MATCH, got: {:?}", results[0].1);
    }

    // ── DIFF ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_diff_returns_structured_paths() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_diff";
        u.mock_response(method, json!(null), json!({"chain": "main", "blocks": 100})).await;
        t.mock_response(method, json!(null), json!({"chain": "test", "blocks": 100})).await;
        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &no_expected()).await;
        assert_eq!(results.len(), 1);
        if let ParityResult::Diff { diff_entries } = &results[0].1 {
            assert_eq!(diff_entries.len(), 1);
            assert_eq!(diff_entries[0].path, "/chain");
        } else {
            panic!("expected Diff, got {:?}", results[0].1);
        }
    }

    // ── EXPECTED_DIFF (method-level) ──────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_expected_diff_method_level_is_labeled() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_expected_diff";
        u.mock_response(method, json!(null), json!({"version": "zcashd/4.7.0"})).await;
        t.mock_response(method, json!(null), json!({"version": "zallet/0.1.0"})).await;

        let expected = ExpectedDiffs {
            expected: vec![ExpectedDiffEntry {
                method: method.to_string(),
                reason: "Zallet reports a different version string.".to_string(),
                diff_paths: vec![],  // method-level: any diff is expected
            }],
        };

        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &expected).await;
        assert_eq!(results.len(), 1);
        if let ParityResult::ExpectedDiff { reason, .. } = &results[0].1 {
            assert!(reason.contains("version"));
        } else {
            panic!("expected ExpectedDiff, got {:?}", results[0].1);
        }
    }

    // ── EXPECTED_DIFF (field-level) covers exact paths ────────────────────────

    #[tokio::test]
    async fn test_parity_expected_diff_field_level_covered() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_field_expected";
        // Diff only at /softforks — which is covered by the expected entry
        u.mock_response(method, json!(null), json!({"chain": "main", "softforks": [{"id": "csv"}]})).await;
        t.mock_response(method, json!(null), json!({"chain": "main", "softforks": []})).await;

        let expected = ExpectedDiffs {
            expected: vec![ExpectedDiffEntry {
                method: method.to_string(),
                reason: "Zallet omits softforks field.".to_string(),
                diff_paths: vec!["/softforks".to_string()],
            }],
        };

        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &expected).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::ExpectedDiff { .. }),
            "covered field diff should be ExpectedDiff, got: {:?}", results[0].1);
    }

    // ── DIFF when only some paths are expected ────────────────────────────────

    #[tokio::test]
    async fn test_parity_unexpected_diff_when_extra_path_differs() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_partial_expected";
        // Diff at /softforks (expected) AND /chain (unexpected)
        u.mock_response(method, json!(null), json!({"chain": "main", "softforks": [{"id": "csv"}]})).await;
        t.mock_response(method, json!(null), json!({"chain": "test", "softforks": []})).await;

        let expected = ExpectedDiffs {
            expected: vec![ExpectedDiffEntry {
                method: method.to_string(),
                reason: "Only softforks is expected.".to_string(),
                diff_paths: vec!["/softforks".to_string()],
            }],
        };

        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &expected).await;
        assert_eq!(results.len(), 1);
        // /chain is NOT covered → must be DIFF, not ExpectedDiff
        assert!(matches!(results[0].1, ParityResult::Diff { .. }),
            "partial coverage should remain DIFF, got: {:?}", results[0].1);
    }

    // ── MATCH via ignore_paths ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_ignore_path_suppresses_diff() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_ignore";
        u.mock_response(method, json!(null), json!({"chain": "main", "volatile": 999})).await;
        t.mock_response(method, json!(null), json!({"chain": "main", "volatile": 888})).await;
        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry_with_ignore(method, vec!["/volatile"])], &no_expected()).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match),
            "ignored path diff should be MATCH, got: {:?}", results[0].1);
    }

    // ── MISSING ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_missing_on_target() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_missing";
        u.mock_response(method, json!(null), json!({"ok": true})).await;
        t.mock_method_not_found(method, json!(null)).await;
        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &no_expected()).await;
        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0].1, ParityResult::Missing { method: m } if m == method),
            "expected Missing, got {:?}", results[0].1
        );
    }

    // ── ERROR ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_error_on_upstream() {
        let u = MockNode::spawn().await;
        let t = MockNode::spawn().await;
        let method = "test_error";
        u.mock_rpc_error(method, json!(null), -32603, "Internal server error").await;
        t.mock_response(method, json!(null), json!({"ok": true})).await;
        let engine = ParityEngine::new(RpcClient::new(&u.url()).unwrap(), RpcClient::new(&t.url()).unwrap());
        let results = engine.run_all(vec![entry(method)], &no_expected()).await;
        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0].1, ParityResult::Error(msg) if msg.contains("Upstream error")),
            "expected Error, got {:?}", results[0].1
        );
    }
}
