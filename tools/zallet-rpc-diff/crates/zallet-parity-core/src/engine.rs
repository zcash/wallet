use tokio::task::JoinSet;
use serde_json::Value;
use crate::client::RpcClient;
use crate::Error;

/// The result of a single parity check.
#[derive(Debug, Clone)]
pub enum ParityResult {
    Match,
    Diff {
        upstream: Value,
        target: Value,
        diff_message: String,
    },
    /// One endpoint returned a JSON-RPC "method not found" error (-32601).
    Missing {
        method: String,
    },
    /// A transport failure or non-missing RPC error occurred.
    Error(String),
}

/// JSON-RPC "method not found" error code per spec.
const METHOD_NOT_FOUND_CODE: i32 = -32601;

/// Classify an RPC call result: transport/RPC errors → Ok(Value) or Err(Error).
/// Returns Err(Error::MethodNotFound) if the server returned -32601.
fn is_method_not_found(err: &Error) -> bool {
    match err {
        Error::JsonRpc(e) => {
            // jsonrpsee surfaces method-not-found as an ErrorObject with code -32601
            e.to_string().contains(&METHOD_NOT_FOUND_CODE.to_string())
                || e.to_string().to_lowercase().contains("method not found")
        }
        _ => false,
    }
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

    /// Runs the parity checks for a list of methods defined in the manifest.
    pub async fn run_all(&self, methods: Vec<crate::manifest::MethodEntry>) -> Vec<(String, ParityResult)> {
        let mut set = JoinSet::new();
        let mut results = Vec::new();

        for entry in methods {
            let upstream = self.upstream.clone();
            let target = self.target.clone();
            let method_name = entry.name.clone();
            let params = entry.params.unwrap_or(Value::Null);

            set.spawn(async move {
                let res_u = upstream.call(&method_name, params.clone()).await;
                let res_t = target.call(&method_name, params).await;

                let parity = match (res_u, res_t) {
                    (Ok(u), Ok(t)) => {
                        let diff = assert_json_diff::assert_json_matches_no_panic(
                            &u,
                            &t,
                            assert_json_diff::Config::new(assert_json_diff::CompareMode::Strict),
                        );

                        match diff {
                            Ok(_) => ParityResult::Match,
                            Err(d) => ParityResult::Diff {
                                upstream: u,
                                target: t,
                                diff_message: d,
                            },
                        }
                    }
                    // Both sides report method-not-found → MISSING on both
                    (Err(ref e_u), Err(ref e_t))
                        if is_method_not_found(e_u) && is_method_not_found(e_t) =>
                    {
                        ParityResult::Missing {
                            method: method_name.clone(),
                        }
                    }
                    // One side reports method-not-found → MISSING (asymmetric)
                    (Err(ref e), _) if is_method_not_found(e) => ParityResult::Missing {
                        method: method_name.clone(),
                    },
                    (_, Err(ref e)) if is_method_not_found(e) => ParityResult::Missing {
                        method: method_name.clone(),
                    },
                    // All other upstream errors
                    (Err(e), _) => ParityResult::Error(format!("Upstream error: {}", e)),
                    // All other target errors
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
        MethodEntry { name: name.to_string(), params: None }
    }

    // ── MATCH ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_match() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_match";
        let response = json!({"blocks": 100});

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

    // ── DIFF ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_diff() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_diff";

        upstream_node.mock_response(method, json!(null), json!({"data": 1})).await;
        target_node.mock_response(method, json!(null), json!({"data": 2})).await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![entry(method)]).await;

        assert_eq!(results.len(), 1);
        let ParityResult::Diff { diff_message, .. } = &results[0].1 else {
            panic!("expected Diff, got {:?}", results[0].1);
        };
        // The diff message should mention the path that changed
        assert!(!diff_message.is_empty());
    }

    // ── MISSING ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_parity_missing_on_target() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_missing";

        // Upstream succeeds; target returns -32601 "method not found"
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

        // Upstream returns a non-method-not-found RPC error (e.g. -32603 internal)
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
