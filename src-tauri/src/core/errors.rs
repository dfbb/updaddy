use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskErrorKind {
    NetworkTimeout,
    ProxyDisconnected,
    Http5xx,
    PermissionDenied,
    CommandFailed,
    InvalidInput,
    Unknown,
}

impl TaskErrorKind {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::NetworkTimeout | Self::ProxyDisconnected | Self::Http5xx
        )
    }
}
