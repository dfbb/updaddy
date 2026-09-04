use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::errors::TaskErrorKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    Homebrew,
    Npm,
    Pip,
    Gem,
    Rustup,
}

impl Ecosystem {
    pub const ALL: [Self; 5] = [
        Self::Homebrew,
        Self::Npm,
        Self::Pip,
        Self::Gem,
        Self::Rustup,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Formula,
    Cask,
    Tap,
    Package,
    Toolchain,
    Component,
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Scan,
    Update,
    Uninstall,
    MeasureDisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskUsageStatus {
    Ready,
    Measuring,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Interrupted,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRecord {
    pub id: String,
    pub ecosystem: Ecosystem,
    pub resource_kind: ResourceKind,
    pub name: String,
    pub current_version: Option<String>,
    pub target_version: Option<String>,
    pub disk_usage: Option<DiskUsage>,
    pub update_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskUsage {
    pub bytes: u64,
    pub scanned_at: i64,
    pub status: DiskUsageStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub message: String,
    pub emitted_at: i64,
    pub stream: String,
}

impl LogEntry {
    pub fn at(message: impl Into<String>, unix_seconds: i64) -> Self {
        Self {
            message: message.into(),
            emitted_at: unix_seconds,
            stream: "system".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTask {
    pub task_id: Uuid,
    pub ecosystem: Ecosystem,
    pub name: String,
    pub operation: Operation,
    pub status: TaskStatus,
    pub error: Option<TaskErrorKind>,
}

impl PackageTask {
    pub fn new(ecosystem: Ecosystem, name: impl Into<String>, operation: Operation) -> Self {
        Self {
            task_id: Uuid::new_v4(),
            ecosystem,
            name: name.into(),
            operation,
            status: TaskStatus::Pending,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationBatch {
    pub batch_id: Uuid,
    pub ecosystem: Ecosystem,
    pub tasks: Vec<PackageTask>,
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_task_serializes_update_and_uninstall_operations() {
        let task = PackageTask::new(Ecosystem::Npm, "typescript", Operation::Update);
        assert_eq!(task.operation, Operation::Update);
        assert_eq!(serde_json::to_value(&task).unwrap()["ecosystem"], "npm");

        let task = PackageTask::new(Ecosystem::Npm, "typescript", Operation::Uninstall);
        assert_eq!(task.operation, Operation::Uninstall);

        let mut task = PackageTask::new(Ecosystem::Npm, "typescript", Operation::Update);
        task.status = TaskStatus::Interrupted;
        assert_eq!(
            serde_json::to_value(&task).unwrap()["status"],
            "interrupted"
        );
    }

    #[test]
    fn task_error_kind_marks_only_transient_errors_retryable() {
        assert!(TaskErrorKind::NetworkTimeout.is_retryable());
        assert!(TaskErrorKind::ProxyDisconnected.is_retryable());
        assert!(TaskErrorKind::HttpServerTemporaryError.is_retryable());
        assert!(!TaskErrorKind::PermissionDenied.is_retryable());
    }
}
