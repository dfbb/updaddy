use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use updaddy_lib::adapters::{package_record, EcosystemAdapter, ExecutorContext};
use updaddy_lib::commands::{submit_update_command, AppState, PackageId};
use updaddy_lib::core::{
    Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind, TaskStatus,
};
use updaddy_lib::disk_usage::PackageInstallPaths;
use updaddy_lib::persistence::Database;
use updaddy_lib::workers::{
    SupervisorContext, WorkerCommand, WorkerEvent, WorkerEventSink, WorkerSupervisor,
};

#[test]
fn update_command_returns_task_id_without_waiting_for_worker_completion() {
    let app = AppState::test_with_blocked_worker();
    let task_id = submit_update_command(&app, PackageId::new(Ecosystem::Npm, "eslint")).unwrap();
    assert_ne!(task_id, uuid::Uuid::nil());
    assert!(app.worker_events().is_empty());
}

struct RetryAdapter {
    scanned: Arc<Mutex<bool>>,
    updated: Arc<Mutex<bool>>,
}

#[async_trait]
impl EcosystemAdapter for RetryAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    async fn scan_with_cancel(
        &self,
        _context: &ExecutorContext,
        _cancel: CancellationToken,
    ) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        *self.scanned.lock().unwrap() = true;
        Ok(vec![package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "eslint",
            Some("1.0.0".into()),
            Some("2.0.0".into()),
        )])
    }

    async fn resolve_install_paths(
        &self,
        _context: &ExecutorContext,
        _cancel: CancellationToken,
    ) -> Result<Vec<std::path::PathBuf>, TaskErrorKind> {
        Ok(Vec::new())
    }

    fn package_install_paths(
        &self,
        _package: &PackageRecord,
        _install_roots: &[std::path::PathBuf],
    ) -> PackageInstallPaths {
        PackageInstallPaths::unavailable(std::path::PathBuf::new())
    }

    async fn update(
        &self,
        _context: &ExecutorContext,
        task: &PackageTask,
        _cancel: CancellationToken,
    ) -> Result<Vec<String>, TaskErrorKind> {
        assert!(*self.scanned.lock().unwrap());
        *self.updated.lock().unwrap() = true;
        Ok(vec![task.name.clone()])
    }
}

#[test]
fn retry_rescans_and_revalidates_before_updating() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::open(directory.path().join("retry.sqlite")).unwrap());
    let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let sink: WorkerEventSink = {
        let events = events.clone();
        Arc::new(move |event| {
            events.0.lock().unwrap().push(event);
            events.1.notify_all();
        })
    };
    let scanned = Arc::new(Mutex::new(false));
    let updated = Arc::new(Mutex::new(false));
    let mut context = SupervisorContext::new(sink);
    context.database = Some(database);
    context.adapters.insert(
        Ecosystem::Npm,
        Arc::new(RetryAdapter {
            scanned: scanned.clone(),
            updated: updated.clone(),
        }),
    );
    let supervisor = WorkerSupervisor::start(context);
    let task = PackageTask::new(Ecosystem::Npm, "eslint", Operation::Update);
    let task_id = supervisor.submit(WorkerCommand::Retry(task)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut guard = events.0.lock().unwrap();
    loop {
        if guard.iter().any(|event| {
            matches!(event, WorkerEvent::TaskProgress { task_id: id, status: TaskStatus::Succeeded, .. } if *id == task_id)
        }) {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "retry task did not complete");
        guard = events.1.wait_timeout(guard, remaining).unwrap().0;
    }
    assert!(guard.iter().any(|event| {
        matches!(
            event,
            WorkerEvent::TaskProgress {
                task_id: id,
                name,
                operation: Operation::Update,
                ..
            } if *id == task_id && name == "eslint"
        )
    }));
    assert!(*scanned.lock().unwrap());
    assert!(*updated.lock().unwrap());
}
