use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::adapters::ExecutorContext;
use crate::core::{Ecosystem, Operation, PackageTask, TaskErrorKind, TaskStatus};
use crate::persistence::Database;

use super::messages::WorkerCommand;
use super::{WorkerEvent, WorkerEventSink};

pub use crate::adapters::EcosystemAdapter;

/// Adapter used when a worker has no implementation yet.
pub struct NoopAdapter;

impl EcosystemAdapter for NoopAdapter {}

const MAX_ATTEMPTS: u32 = 3;

async fn run_with_retries(
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    command: WorkerCommand,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    task_id: Uuid,
) -> Result<(), TaskErrorKind> {
    let mut attempt = 1_u32;
    loop {
        let started_at = Utc::now().timestamp();
        let result = adapter
            .run_with_context(executor, command.clone(), cancel.clone())
            .await;
        let status = match &result {
            Ok(()) => "succeeded",
            Err(_) if cancel.is_cancelled() => "cancelled",
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => "retrying",
            Err(_) => "failed",
        };
        if let Some(database) = database {
            let _ = database.record_task_attempt(
                task_id,
                attempt,
                status,
                Some(started_at),
                Some(Utc::now().timestamp()),
            );
        }
        match result {
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                let delay = Duration::from_secs(1_u64 << (attempt - 1));
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => return Err(TaskErrorKind::CommandFailed),
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            result => return result,
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
    sequence: &mut u64,
    executor: ExecutorContext,
) {
    *sequence += 1;
    emit_state(ecosystem, "running", *sequence, &sink);
    *sequence += 1;
    emit_progress(
        task_id,
        ecosystem,
        *sequence,
        TaskStatus::Running,
        None,
        &sink,
    );
    if let Some(database) = &database {
        if !matches!(
            database.update_task(task_id, TaskStatus::Running, None),
            Ok(true)
        ) {
            *sequence += 1;
            emit_progress(
                task_id,
                ecosystem,
                *sequence,
                TaskStatus::Failed,
                Some(TaskErrorKind::Unknown),
                &sink,
            );
            *sequence += 1;
            emit_state(ecosystem, "idle", *sequence, &sink);
            return;
        }
    }

    let result = if cancel.is_cancelled() {
        Err(TaskErrorKind::CommandFailed)
    } else {
        run_with_retries(
            adapter.as_ref(),
            &executor,
            command,
            cancel.clone(),
            database.as_ref(),
            task_id,
        )
        .await
    };

    let (status, error) = if cancel.is_cancelled() {
        (TaskStatus::Cancelled, None)
    } else {
        match result {
            Ok(()) => (TaskStatus::Succeeded, None),
            Err(error) => (TaskStatus::Failed, Some(error)),
        }
    };
    *sequence += 1;
    emit_progress(task_id, ecosystem, *sequence, status, error, &sink);
    if let Some(database) = &database {
        if !matches!(database.update_task(task_id, status, error), Ok(true)) {
            *sequence += 1;
            emit_progress(
                task_id,
                ecosystem,
                *sequence,
                TaskStatus::Failed,
                Some(TaskErrorKind::Unknown),
                &sink,
            );
        }
    }
    *sequence += 1;
    emit_state(ecosystem, "idle", *sequence, &sink);
}

pub(crate) fn command_task(
    task_id: Uuid,
    ecosystem: Ecosystem,
    command: &WorkerCommand,
) -> PackageTask {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_adapter_is_explicitly_unavailable() {
        let result = NoopAdapter
            .run(
                WorkerCommand::Scan(Ecosystem::Homebrew),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(TaskErrorKind::Unknown)));
    }
}

fn emit_state(ecosystem: Ecosystem, state: &str, sequence: u64, sink: &WorkerEventSink) {
    sink(WorkerEvent::WorkerState {
        ecosystem,
        sequence,
        state: state.to_owned(),
        emitted_at: Utc::now().timestamp(),
    });
}

fn emit_progress(
    task_id: Uuid,
    ecosystem: Ecosystem,
    sequence: u64,
    status: TaskStatus,
    error: Option<TaskErrorKind>,
    sink: &WorkerEventSink,
) {
    sink(WorkerEvent::TaskProgress {
        task_id,
        ecosystem,
        sequence,
        status,
        completed: u64::from(matches!(
            status,
            TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled
        )),
        total: 1,
        message: None,
        error,
        emitted_at: Utc::now().timestamp(),
    });
}
