use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonrpsee::core::RpcResult;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

/// The possible states that an async operation can be in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(into = "&'static str")]
pub(super) enum OperationState {
    Ready,
    Executing,
    Cancelled,
    Failed,
    Success,
}

impl OperationState {
    pub(super) fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(Self::Ready),
            "executing" => Some(Self::Executing),
            "cancelled" => Some(Self::Cancelled),
            "failed" => Some(Self::Failed),
            "success" => Some(Self::Success),
            _ => None,
        }
    }
}

impl From<OperationState> for &'static str {
    fn from(value: OperationState) -> Self {
        match value {
            OperationState::Ready => "queued",
            OperationState::Executing => "executing",
            OperationState::Cancelled => "cancelled",
            OperationState::Failed => "failed",
            OperationState::Success => "success",
        }
    }
}

/// Data associated with an async operation.
pub(super) struct OperationData {
    state: OperationState,
    start_time: Option<SystemTime>,
    end_time: Option<SystemTime>,
    result: Option<RpcResult<Value>>,
}

/// An async operation launched by an RPC call.
pub(super) struct AsyncOperation {
    operation_id: String,
    creation_time: SystemTime,
    data: Arc<RwLock<OperationData>>,
}

impl AsyncOperation {
    /// Launches a new async operation.
    pub(super) async fn new<T: Serialize + Send + 'static>(
        f: impl Future<Output = RpcResult<T>> + Send + 'static,
    ) -> Self {
        let creation_time = SystemTime::now();

        let data = Arc::new(RwLock::new(OperationData {
            state: OperationState::Ready,
            start_time: None,
            end_time: None,
            result: None,
        }));

        let handle = data.clone();

        tokio::spawn(async move {
            // Record that the task has started.
            {
                let mut data = handle.write().await;
                if matches!(data.state, OperationState::Cancelled) {
                    return;
                }
                data.state = OperationState::Executing;
                data.start_time = Some(SystemTime::now());
            }

            // Run the async task.
            let res = f.await;
            let end_time = SystemTime::now();

            // Map the concrete task result into a generic JSON blob.
            let res = res.map(|ret| {
                serde_json::from_str(
                    &serde_json::to_string(&ret)
                        .expect("async return values should be serializable to JSON"),
                )
                .expect("round trip should succeed")
            });

            // Record the result.
            let mut data = handle.write().await;
            data.state = if res.is_ok() {
                OperationState::Success
            } else {
                OperationState::Failed
            };
            data.end_time = Some(end_time);
            data.result = Some(res);
        });

        Self {
            operation_id: format!("opid-{}", Uuid::new_v4()),
            creation_time,
            data,
        }
    }

    /// Returns the ID of this operation.
    pub(super) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Returns the current state of this operation.
    pub(super) async fn state(&self) -> OperationState {
        self.data.read().await.state
    }

    /// Builds the current status of this operation.
    pub(super) async fn to_status(&self) -> OperationStatus {
        let data = self.data.read().await;

        let creation_time = self
            .creation_time
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let (error, result, execution_secs) = match &data.result {
            None => (None, None, None),
            Some(Err(e)) => (
                Some(OperationError {
                    code: e.code(),
                    message: e.message().to_string(),
                    data: e.data().map(|data| data.get().to_string()),
                }),
                None,
                None,
            ),
            Some(Ok(v)) => (
                None,
                Some(v.clone()),
                data.end_time.zip(data.start_time).map(|(end, start)| {
                    end.duration_since(start)
                        .ok()
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                }),
            ),
        };

        OperationStatus {
            id: self.operation_id.clone(),
            status: data.state,
            creation_time,
            error,
            result,
            execution_secs,
        }
    }
}

/// The status of an async operation.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct OperationStatus {
    id: String,

    status: OperationState,

    // The creation time, in seconds since the Unix epoch.
    creation_time: u64,

    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<OperationError>,

    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,

    /// Execution time for successful operations.
    #[serde(skip_serializing_if = "Option::is_none")]
    execution_secs: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct OperationError {
    /// Code
    code: i32,

    /// Message
    message: String,

    /// Optional data
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}
