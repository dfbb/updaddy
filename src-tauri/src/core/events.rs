use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::errors::TaskErrorKind;
use super::models::{DiskUsage, Ecosystem, LogEntry, PackageRecord, TaskStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerStateEvent {
    pub ecosystem: Ecosystem,
    pub sequence: u64,
    pub emitted_at: i64,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProgressEvent {
    pub task_id: Uuid,
    pub ecosystem: Ecosystem,
    pub sequence: u64,
    pub emitted_at: i64,
    pub status: TaskStatus,
    pub completed: u64,
    pub total: u64,
    pub message: Option<String>,
    pub error: Option<TaskErrorKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChangedEvent {
    pub task_id: Uuid,
    pub ecosystem: Ecosystem,
    pub sequence: u64,
    pub emitted_at: i64,
    pub package: PackageRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskUsageEvent {
    pub task_id: Uuid,
    pub ecosystem: Ecosystem,
    pub sequence: u64,
    pub emitted_at: i64,
    pub package_id: String,
    pub disk_usage: DiskUsage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntryEvent {
    pub task_id: Uuid,
    pub ecosystem: Ecosystem,
    pub sequence: u64,
    pub emitted_at: i64,
    pub entry: LogEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchSummaryEvent {
    pub batch_id: Uuid,
    pub ecosystem: Ecosystem,
    pub sequence: u64,
    pub emitted_at: i64,
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
}
