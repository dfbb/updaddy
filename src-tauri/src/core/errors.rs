use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorKind {
    NetworkTimeout,
    ProxyDisconnected,
    HttpServerTemporaryError,
    PermissionDenied,
    CommandFailed,
    InvalidInput,
    Unknown,
}

impl TaskErrorKind {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::NetworkTimeout | Self::ProxyDisconnected | Self::HttpServerTemporaryError
        )
    }
}
