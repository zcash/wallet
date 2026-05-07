use tokio::task::JoinSet;
use serde_json::Value;
use crate::client::RpcClient;
use crate::differ::{diff_values, DiffEntry};
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
    /// Both endpoints returned data, but the normalized values differ.
    Diff {
        /// Structured list of leaf-level differences (with JSON Pointer paths).
        diff_entries: Vec<DiffEntry>,
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
    /// before comparison.
    pub async fn run_all(&self, methods: Vec<crate::manifest::MethodEntry>) -> Vec<(String, ParityResult)> {
        let mut set = JoinSet::new();
        let mut results = Vec::new();

        for entry in methods {
            let upstream = self.upstream.clone();
            let target = self.target.clone();
            let method_name = entry.name.clone();
            let params = entry.params.unwrap_or(Value::Null);
            let raw_ignore_paths = entry.ignore_paths.clone();

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
                            ParityResult::Diff { diff_entries: entries }
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
    use crate::manifest::MethodEntry;
    use serde_json::json;

    fn entry(name: &str) -> MethodEntry {
        MethodEntry {
            name: name.to_string(),
            params: None,
            ignore_paths: vec![],
        }
    }

    fn entry_with_ignore(name: &str, paths: Vec<&str>) -> MethodEntry {
        MethodEntry {
            name: name.to_string(),
            params: None,
            ignore_paths: paths.into_iter().map(String::from).collect(),
        }
    }

    // ── MATCH ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_match() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_match";
        let response = json!({"blocks": 100, "chain": "main"});

        upstream_node.mock_response(method, json!(null), response.clone()).await;
        target_node.mock_response(method, json!(null), response).await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![entry(method)]).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match));
    }

    // ── MATCH via normalization (ordering-only diff) ───────────────────────────

    #[tokio::test]
    async fn test_parity_ordering_only_diff_is_match_after_normalization() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_ordering";
        upstream_node
            .mock_response(method, json!(null), json!({"z": 1, "a": 2, "m": 3}))
            .await;
        target_node
            .mock_response(method, json!(null), json!({"a": 2, "m": 3, "z": 1}))
            .await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![entry(method)]).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match),
            "ordering-only diff should be MATCH after normalization, got: {:?}", results[0].1);
    }

    // ── DIFF with structured paths ────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_diff_returns_structured_paths() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_diff";
        upstream_node
            .mock_response(method, json!(null), json!({"chain": "main", "blocks": 100}))
            .await;
        target_node
            .mock_response(method, json!(null), json!({"chain": "test", "blocks": 100}))
            .await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![entry(method)]).await;

        assert_eq!(results.len(), 1);
        if let ParityResult::Diff { diff_entries } = &results[0].1 {
            assert_eq!(diff_entries.len(), 1);
            assert_eq!(diff_entries[0].path, "/chain");
        } else {
            panic!("expected Diff, got {:?}", results[0].1);
        }
    }

    // ── MATCH via ignore_paths ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_ignore_path_suppresses_diff() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_ignore";
        upstream_node
            .mock_response(method, json!(null), json!({"chain": "main", "volatile": 999}))
            .await;
        target_node
            .mock_response(method, json!(null), json!({"chain": "main", "volatile": 888}))
            .await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine
            .run_all(vec![entry_with_ignore(method, vec!["/volatile"])])
            .await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match),
            "diff only at ignored path should be MATCH, got: {:?}", results[0].1);
    }

    // ── MISSING ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_missing_on_target() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_missing";

        upstream_node.mock_response(method, json!(null), json!({"ok": true})).await;
        target_node.mock_method_not_found(method, json!(null)).await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![entry(method)]).await;

        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0].1, ParityResult::Missing { method: m } if m == method),
            "expected Missing, got {:?}", results[0].1
        );
    }

    // ── ERROR ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_error_on_upstream() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_error";

        upstream_node.mock_rpc_error(method, json!(null), -32603, "Internal server error").await;
        target_node.mock_response(method, json!(null), json!({"ok": true})).await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![entry(method)]).await;

        assert_eq!(results.len(), 1);
        assert!(
            matches!(&results[0].1, ParityResult::Error(msg) if msg.contains("Upstream error")),
            "expected Error, got {:?}", results[0].1
        );
    }
}
