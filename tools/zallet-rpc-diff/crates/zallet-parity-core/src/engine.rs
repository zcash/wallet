use tokio::task::JoinSet;
use serde_json::Value;
use crate::client::RpcClient;

/// The result of a single parity check.
#[derive(Debug, Clone)]
pub enum ParityResult {
    Match,
    Diff {
        upstream: Value,
        target: Value,
        diff_message: String,
    },
    Missing {
        method: String,
    },
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
                                diff_message: d 
                            },
                        }
                    }
                    (Err(e), _) => ParityResult::Error(format!("Upstream error: {}", e)),
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

    #[tokio::test]
    async fn test_parity_match() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_method";
        let params = json!({"hello": "world"});
        let response = json!({"result": "ok"});

        upstream_node.mock_response(method, params.clone(), response.clone()).await;
        target_node.mock_response(method, params.clone(), response).await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![MethodEntry {
            name: method.to_string(),
            params: Some(params),
        }]).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Match));
    }

    #[tokio::test]
    async fn test_parity_diff() {
        let upstream_node = MockNode::spawn().await;
        let target_node = MockNode::spawn().await;

        let method = "test_method";
        let params = json!({"hello": "world"});
        
        upstream_node.mock_response(method, params.clone(), json!({"data": 1})).await;
        target_node.mock_response(method, params.clone(), json!({"data": 2})).await;

        let engine = ParityEngine::new(
            RpcClient::new(&upstream_node.url()).unwrap(),
            RpcClient::new(&target_node.url()).unwrap(),
        );

        let results = engine.run_all(vec![MethodEntry {
            name: method.to_string(),
            params: Some(params),
        }]).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].1, ParityResult::Diff { .. }));
    }
}
