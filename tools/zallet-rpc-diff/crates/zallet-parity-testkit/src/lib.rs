use wiremock::{MockServer, Mock, ResponseTemplate, Request};
use wiremock::matchers::{method, path};
use serde_json::{json, Value};

/// A mock Zcash RPC node for testing.
pub struct MockNode {
    server: MockServer,
}

impl MockNode {
    /// Spawns a new mock node on a random port.
    pub async fn spawn() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// Returns the RPC URL for this node.
    pub fn url(&self) -> String {
        self.server.uri()
    }

    /// Mocks a successful RPC response for a specific method and params.
    /// It matches if the method name is correct and the params array contains the expected value.
    pub async fn mock_response(&self, method_name: &str, expected_params: Value, result: Value) {
        let method_name = method_name.to_string();
        
        Mock::given(method("POST"))
            .and(path("/"))
            .and(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let m = body.get("method").and_then(|v| v.as_str()) == Some(&method_name);
                
                // jsonrpsee wraps params in a list: [ { ... } ]
                let p = body.get("params")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first()) == Some(&expected_params);
                
                m && p
            })
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let id = body.get("id").cloned().unwrap_or(json!(1));
                
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "result": result,
                    "id": id
                }))
            })
            .mount(&self.server)
            .await;
    }

    /// Mocks an RPC error response.
    pub async fn mock_error(&self, method_name: &str, expected_params: Value, code: i32, error_message: &str) {
        let method_name = method_name.to_string();
        let error_message = error_message.to_string();

        Mock::given(method("POST"))
            .and(path("/"))
            .and(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let m = body.get("method").and_then(|v| v.as_str()) == Some(&method_name);
                let p = body.get("params")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first()) == Some(&expected_params);
                m && p
            })
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let id = body.get("id").cloned().unwrap_or(json!(1));
                
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": code,
                        "message": error_message
                    },
                    "id": id
                }))
            })
            .mount(&self.server)
            .await;
    }
}
