mod messages;
mod supervisor;
mod worker;

pub use messages::WorkerCommand;
pub use supervisor::{
    resource_lock_key, ResourceLock, Result, SupervisorContext, WorkerContext, WorkerSupervisor,
};
pub use worker::{EcosystemAdapter, NoopAdapter};

use crate::core::{Ecosystem, TaskErrorKind, TaskStatus};
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

pub type TaskId = Uuid;

#[derive(Debug, Clone, Serialize)]
pub enum WorkerEvent {
    WorkerState {
        ecosystem: Ecosystem,
        sequence: u64,
        state: String,
        emitted_at: i64,
    },
    TaskProgress {
        task_id: Uuid,
        ecosystem: Ecosystem,
        sequence: u64,
        status: TaskStatus,
        completed: u64,
        total: u64,
        message: Option<String>,
        error: Option<TaskErrorKind>,
        emitted_at: i64,
    },
}

pub type WorkerEventSink = Arc<dyn Fn(WorkerEvent) + Send + Sync + 'static>;
