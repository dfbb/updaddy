use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::{Ecosystem, Operation, OperationBatch, PackageTask, TaskErrorKind, TaskStatus};
use crate::persistence::Database;

use super::messages::WorkerCommand;
use super::{WorkerEvent, WorkerEventSink};

/// Minimal adapter contract. Concrete package-manager behavior belongs to Task 6.
#[async_trait]
pub trait EcosystemAdapter: Send + Sync + 'static {
    async fn run(
        &self,
        command: WorkerCommand,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind>;
}

/// Adapter used when a worker has no implementation yet.
pub struct NoopAdapter;

#[async_trait]
impl EcosystemAdapter for NoopAdapter {
    async fn run(
        &self,
        _command: WorkerCommand,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        if cancel.is_cancelled() {
            Err(TaskErrorKind::CommandFailed)
        } else {
            Ok(())
        }
    }
}

pub(crate) async fn run_command(
    ecosystem: Ecosystem,
    task_id: Uuid,
    command: WorkerCommand,
    adapter: Arc<dyn EcosystemAdapter>,
    cancel: CancellationToken,
    sink: WorkerEventSink,
    database: Option<Arc<Database>>,
) {
    if let Some(database) = &database {
        let task = command_task(task_id, ecosystem, &command);
        let batch = OperationBatch {
            batch_id: Uuid::new_v4(),
            ecosystem,
            tasks: vec![task],
            created_at: Utc::now().timestamp(),
        };
        let _ = database.create_batch(&batch);
    }
    emit_state(ecosystem, "running", &sink);
    emit_progress(task_id, ecosystem, TaskStatus::Running, None, &sink);
    if let Some(database) = &database {
        let _ = database.update_task(task_id, TaskStatus::Running, None);
    }

    let result = if cancel.is_cancelled() {
        Err(TaskErrorKind::CommandFailed)
    } else {
        adapter.run(command, cancel.clone()).await
    };

    let (status, error) = if cancel.is_cancelled() {
        (TaskStatus::Cancelled, None)
    } else {
        match result {
            Ok(()) => (TaskStatus::Succeeded, None),
            Err(error) => (TaskStatus::Failed, Some(error)),
        }
    };
    emit_progress(task_id, ecosystem, status, error, &sink);
    if let Some(database) = &database {
        let _ = database.update_task(task_id, status, error);
    }
    emit_state(ecosystem, "idle", &sink);
}

fn command_task(task_id: Uuid, ecosystem: Ecosystem, command: &WorkerCommand) -> PackageTask {
    let (name, operation) = match command {
        WorkerCommand::Scan(_) => ("*".to_owned(), Operation::Scan),
        WorkerCommand::RefreshDiskUsage(_) => ("*".to_owned(), Operation::MeasureDisk),
        WorkerCommand::Update(task) => (task.name.clone(), Operation::Update),
        WorkerCommand::Uninstall(task) => (task.name.clone(), Operation::Uninstall),
        WorkerCommand::Shutdown => ("*".to_owned(), Operation::Scan),
    };
    PackageTask {
        task_id,
        ecosystem,
        name,
        operation,
        status: TaskStatus::Pending,
        error: None,
    }
}

fn emit_state(ecosystem: Ecosystem, state: &str, sink: &WorkerEventSink) {
    sink(WorkerEvent::WorkerState {
        ecosystem,
        state: state.to_owned(),
        emitted_at: Utc::now().timestamp(),
    });
}

fn emit_progress(
    task_id: Uuid,
    ecosystem: Ecosystem,
    status: TaskStatus,
    error: Option<TaskErrorKind>,
    sink: &WorkerEventSink,
) {
    sink(WorkerEvent::TaskProgress {
        task_id,
        ecosystem,
        status,
        error,
        emitted_at: Utc::now().timestamp(),
    });
}
