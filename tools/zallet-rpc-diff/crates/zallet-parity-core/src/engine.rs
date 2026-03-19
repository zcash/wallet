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
