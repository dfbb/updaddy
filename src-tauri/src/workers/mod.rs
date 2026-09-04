mod messages;
mod supervisor;
mod worker;

pub use messages::WorkerCommand;
pub use supervisor::{
    resource_lock_key, Result, SupervisorContext, WorkerContext, WorkerSupervisor,
};
pub use worker::{EcosystemAdapter, NoopAdapter};

use crate::core::{Ecosystem, TaskErrorKind, TaskStatus};
use std::sync::Arc;
use uuid::Uuid;

pub type TaskId = Uuid;

#[derive(Debug, Clone)]
pub enum WorkerEvent {
    WorkerState {
        ecosystem: Ecosystem,
        state: String,
        emitted_at: i64,
    },
    TaskProgress {
        task_id: Uuid,
        ecosystem: Ecosystem,
        status: TaskStatus,
        error: Option<TaskErrorKind>,
        emitted_at: i64,
    },
}

pub type WorkerEventSink = Arc<dyn Fn(WorkerEvent) + Send + Sync + 'static>;
