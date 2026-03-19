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

    /// Runs the parity checks for a list of methods.
    pub async fn run_all(&self, methods: Vec<String>) -> Vec<(String, ParityResult)> {
        let mut set = JoinSet::new();
        let mut results = Vec::new();

        for method in methods {
            let upstream = self.upstream.clone();
            let target = self.target.clone();
            let m = method.clone();

            set.spawn(async move {
                let res_u = upstream.call(&m, Value::Null).await;
                let res_t = target.call(&m, Value::Null).await;

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

                (m, parity)
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
