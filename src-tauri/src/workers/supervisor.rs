use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use tokio::runtime::Builder;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::Ecosystem;
use crate::core::{OperationBatch, TaskStatus};
use crate::persistence::Database;

use super::messages::WorkerCommand;
use super::worker::{run_command, EcosystemAdapter, NoopAdapter};
use super::{WorkerEvent, WorkerEventSink};

#[derive(Clone)]
pub struct SupervisorContext {
    pub adapters: HashMap<Ecosystem, Arc<dyn EcosystemAdapter>>,
    pub database: Option<Arc<Database>>,
    pub event_sink: WorkerEventSink,
}

pub type WorkerContext = SupervisorContext;

impl SupervisorContext {
    pub fn new(event_sink: WorkerEventSink) -> Self {
        Self {
            adapters: HashMap::new(),
            database: None,
            event_sink,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("worker for {0:?} is unavailable")]
    Unavailable(Ecosystem),
    #[error("task {0} is not active")]
    UnknownTask(Uuid),
    #[error("task {0} is already active")]
    DuplicateTask(Uuid),
}

pub type Result<T> = std::result::Result<T, SupervisorError>;

struct Envelope {
    id: Uuid,
    command: WorkerCommand,
    cancel: CancellationToken,
}

pub struct WorkerSupervisor {
    senders: HashMap<Ecosystem, mpsc::Sender<Envelope>>,
    cancellations: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    database: Option<Arc<Database>>,
}

impl WorkerSupervisor {
    pub fn start(context: SupervisorContext) -> Self {
        let cancellations = Arc::new(Mutex::new(HashMap::new()));
        let mut senders = HashMap::new();
        let mut handles = Vec::new();
        for ecosystem in Ecosystem::ALL {
            let (tx, rx) = mpsc::channel::<Envelope>();
            let adapter = context
                .adapters
                .get(&ecosystem)
                .cloned()
                .unwrap_or_else(|| Arc::new(NoopAdapter));
            let sink = context.event_sink.clone();
            let database = context.database.clone();
            let active = cancellations.clone();
            let lock = shared_resource_lock(resource_lock_key(ecosystem));
            context.event_sink(WorkerEvent::WorkerState {
                ecosystem,
                sequence: 0,
                state: "idle".into(),
                emitted_at: chrono::Utc::now().timestamp(),
            });
            let handle = thread::spawn(move || {
                let runtime = Builder::new_current_thread().enable_all().build();
                let Ok(runtime) = runtime else {
                    sink(WorkerEvent::WorkerState {
                        ecosystem,
                        sequence: 1,
                        state: "failed".into(),
                        emitted_at: chrono::Utc::now().timestamp(),
                    });
                    return;
                };
                let mut sequence = 0;
                while let Ok(envelope) = rx.recv() {
                    if envelope.command.ecosystem().is_none() {
                        break;
                    }
                    let id = envelope.id;
                    let command = envelope.command;
                    let cancel = envelope.cancel;
                    if cancel.is_cancelled() {
                        let update = database.as_ref().map_or(Ok(true), |database| {
                            database.update_task(id, TaskStatus::Cancelled, None)
                        });
                        if !matches!(&update, Ok(true)) {
                            sink(WorkerEvent::TaskProgress {
                                task_id: id,
                                ecosystem,
                                sequence: {
                                    sequence += 1;
                                    sequence
                                },
                                status: TaskStatus::Failed,
                                completed: 1,
                                total: 1,
                                message: Some(
                                    match update {
                                        Ok(false) => "database task missing",
                                        Err(_) => "database update failed",
                                        Ok(true) => "",
                                    }
                                    .into(),
                                ),
                                error: Some(crate::core::TaskErrorKind::Unknown),
                                emitted_at: chrono::Utc::now().timestamp(),
                            });
                        } else {
                            sink(WorkerEvent::TaskProgress {
                                task_id: id,
                                ecosystem,
                                sequence: {
                                    sequence += 1;
                                    sequence
                                },
                                status: crate::core::TaskStatus::Cancelled,
                                completed: 1,
                                total: 1,
                                message: None,
                                error: None,
                                emitted_at: chrono::Utc::now().timestamp(),
                            });
                        }
                        active.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                        continue;
                    }
                    let _guard = lock.acquire();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        runtime.block_on(run_command(
                            ecosystem,
                            id,
                            command,
                            adapter.clone(),
                            cancel,
                            sink.clone(),
                            database.clone(),
                            &mut sequence,
                        ))
                    }));
                    if result.is_err() {
                        if let Some(database) = &database {
                            let _ = database.update_task(
                                id,
                                TaskStatus::Failed,
                                Some(crate::core::TaskErrorKind::Unknown),
                            );
                        }
                        sink(WorkerEvent::TaskProgress {
                            task_id: id,
                            ecosystem,
                            sequence: {
                                sequence += 1;
                                sequence
                            },
                            status: TaskStatus::Failed,
                            completed: 1,
                            total: 1,
                            message: Some("worker panicked".into()),
                            error: Some(crate::core::TaskErrorKind::Unknown),
                            emitted_at: chrono::Utc::now().timestamp(),
                        });
                        sink(WorkerEvent::WorkerState {
                            ecosystem,
                            sequence: {
                                sequence += 1;
                                sequence
                            },
                            state: "idle".into(),
                            emitted_at: chrono::Utc::now().timestamp(),
                        });
                    }
                    active.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                }
            });
            senders.insert(ecosystem, tx);
            handles.push(handle);
        }
        Self {
            senders,
            cancellations,
            handles: Mutex::new(handles),
            database: context.database,
        }
    }

    pub fn submit(&self, command: WorkerCommand) -> Result<Uuid> {
        if matches!(command, WorkerCommand::Shutdown) {
            let id = Uuid::new_v4();
            for sender in self.senders.values() {
                let _ = sender.send(Envelope {
                    id,
                    command: WorkerCommand::Shutdown,
                    cancel: CancellationToken::new(),
                });
            }
            return Ok(id);
        }
        let ecosystem = command
            .ecosystem()
            .ok_or(SupervisorError::Unavailable(Ecosystem::Homebrew))?;
        let id = command.task_id().unwrap_or_else(Uuid::new_v4);
        let cancel = CancellationToken::new();
        {
            let mut active = self.cancellations.lock().unwrap_or_else(|p| p.into_inner());
            if active.contains_key(&id) {
                return Err(SupervisorError::DuplicateTask(id));
            }
            active.insert(id, cancel.clone());
        }
        let sender = self
            .senders
            .get(&ecosystem)
            .ok_or(SupervisorError::Unavailable(ecosystem))?;
        // Persist the pending task before making it cancellable/visible to workers.
        if let Some(database) = &self.database {
            let task = super::worker::command_task(id, ecosystem, &command);
            let batch = OperationBatch {
                batch_id: Uuid::new_v4(),
                ecosystem,
                tasks: vec![task],
                created_at: chrono::Utc::now().timestamp(),
            };
            if database.create_batch(&batch).is_err() {
                self.cancellations
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                return Err(SupervisorError::Unavailable(ecosystem));
            }
        }
        sender
            .send(Envelope {
                id,
                command,
                cancel: cancel.clone(),
            })
            .map_err(|_| {
                self.cancellations
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                if let Some(database) = &self.database {
                    let _ = database.update_task(
                        id,
                        TaskStatus::Failed,
                        Some(crate::core::TaskErrorKind::Unknown),
                    );
                }
                SupervisorError::Unavailable(ecosystem)
            })?;
        Ok(id)
    }

    pub fn cancel(&self, task_id: Uuid) -> Result<()> {
        let token = self
            .cancellations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&task_id)
            .cloned()
            .ok_or(SupervisorError::UnknownTask(task_id))?;
        token.cancel();
        Ok(())
    }

    pub fn active_task_count(&self) -> usize {
        self.cancellations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }
}

impl Drop for WorkerSupervisor {
    fn drop(&mut self) {
        for token in self
            .cancellations
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
        {
            token.cancel();
        }
        for sender in self.senders.values() {
            let _ = sender.send(Envelope {
                id: Uuid::new_v4(),
                command: WorkerCommand::Shutdown,
                cancel: CancellationToken::new(),
            });
        }
        // JoinHandle is intentionally dropped after cancellation. Dropping detaches
        // a misbehaving adapter instead of blocking application shutdown forever.
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

pub fn resource_lock_key(ecosystem: Ecosystem) -> &'static str {
    match ecosystem {
        Ecosystem::Homebrew => "brew",
        Ecosystem::Npm => "node-global",
        Ecosystem::Pip => "python-global",
        Ecosystem::Gem => "ruby-global",
        Ecosystem::Rustup => "rustup",
    }
}

/// A named process-wide resource lock. Workers currently own one lock each;
/// the named type keeps serialization explicit for adapters that share a scope.
pub struct ResourceLock {
    key: &'static str,
    gate: Mutex<()>,
}

static RESOURCE_LOCKS: OnceLock<Mutex<HashMap<&'static str, Arc<ResourceLock>>>> = OnceLock::new();

fn shared_resource_lock(key: &'static str) -> Arc<ResourceLock> {
    let locks = RESOURCE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .entry(key)
        .or_insert_with(|| Arc::new(ResourceLock::new(key)))
        .clone()
}

impl ResourceLock {
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            gate: Mutex::new(()),
        }
    }

    pub fn key(&self) -> &'static str {
        self.key
    }
    pub fn acquire(&self) -> std::sync::MutexGuard<'_, ()> {
        self.gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::time::sleep;

    use super::super::{WorkerEvent, WorkerEventSink};
    use crate::core::{Operation, PackageTask, TaskErrorKind, TaskStatus};

    #[derive(Clone)]
    struct TestEvents {
        state: Arc<(Mutex<Vec<WorkerEvent>>, std::sync::Condvar)>,
        release: Arc<AtomicBool>,
    }

    impl TestEvents {
        fn sink(&self) -> WorkerEventSink {
            let state = self.state.clone();
            Arc::new(move |event| {
                let (events, wake) = &*state;
                events.lock().unwrap().push(event);
                wake.notify_all();
            })
        }

        fn has_status(&self, id: Uuid, status: TaskStatus) -> bool {
            self.state.0.lock().unwrap().iter().any(|event| {
                matches!(event, WorkerEvent::TaskProgress { task_id, status: actual, .. } if *task_id == id && *actual == status)
            })
        }

        fn wait_for_status(&self, id: Uuid, status: TaskStatus) {
            let (events, wake) = &*self.state;
            let mut guard = events.lock().unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while !guard.iter().any(|event| matches!(event, WorkerEvent::TaskProgress { task_id, status: actual, .. } if *task_id == id && *actual == status)) {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!remaining.is_zero(), "timed out waiting for task event");
                let (next, result) = wake.wait_timeout(guard, remaining).unwrap();
                guard = next;
                assert!(!result.timed_out(), "timed out waiting for task event");
            }
        }

        fn wait_for_running(&self, id: Uuid) {
            self.wait_for_status(id, TaskStatus::Running);
        }
        fn wait_for_completed(&self, id: Uuid) {
            self.wait_for_status(id, TaskStatus::Succeeded);
        }
        fn wait_for_started(&self, id: Uuid) {
            self.wait_for_running(id);
        }
        fn has_started(&self, id: Uuid) -> bool {
            self.has_status(id, TaskStatus::Running)
        }
        fn release_first_task(&self) {
            self.release.store(true, Ordering::Release);
        }
    }

    struct BarrierAdapter {
        barrier: Arc<std::sync::Barrier>,
    }
    #[async_trait]
    impl EcosystemAdapter for BarrierAdapter {
        async fn run(
            &self,
            _command: WorkerCommand,
            _cancel: CancellationToken,
        ) -> std::result::Result<(), TaskErrorKind> {
            self.barrier.wait();
            Ok(())
        }
    }

    struct DelayedAdapter {
        release: Arc<AtomicBool>,
    }
    #[async_trait]
    impl EcosystemAdapter for DelayedAdapter {
        async fn run(
            &self,
            _command: WorkerCommand,
            cancel: CancellationToken,
        ) -> std::result::Result<(), TaskErrorKind> {
            while !self.release.load(Ordering::Acquire) {
                if cancel.is_cancelled() {
                    return Err(TaskErrorKind::CommandFailed);
                }
                sleep(Duration::from_millis(5)).await;
            }
            Ok(())
        }
    }

    fn package_task(ecosystem: Ecosystem, name: &str) -> PackageTask {
        PackageTask::new(ecosystem, name, Operation::Update)
    }

    #[test]
    fn different_ecosystem_workers_can_run_at_the_same_time() {
        let events = TestEvents {
            state: Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
            release: Arc::new(AtomicBool::new(false)),
        };
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut adapters = HashMap::new();
        adapters.insert(
            Ecosystem::Homebrew,
            Arc::new(BarrierAdapter {
                barrier: barrier.clone(),
            }) as Arc<dyn EcosystemAdapter>,
        );
        adapters.insert(
            Ecosystem::Npm,
            Arc::new(BarrierAdapter { barrier }) as Arc<dyn EcosystemAdapter>,
        );
        let supervisor = WorkerSupervisor::start(SupervisorContext {
            adapters,
            database: None,
            event_sink: events.sink(),
        });
        let brew = supervisor
            .submit(WorkerCommand::Scan(Ecosystem::Homebrew))
            .unwrap();
        let npm = supervisor
            .submit(WorkerCommand::Scan(Ecosystem::Npm))
            .unwrap();
        events.wait_for_running(brew);
        events.wait_for_running(npm);
        events.wait_for_completed(brew);
        events.wait_for_completed(npm);
    }

    #[test]
    fn same_ecosystem_commands_are_serialized() {
        let events = TestEvents {
            state: Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
            release: Arc::new(AtomicBool::new(false)),
        };
        let mut adapters = HashMap::new();
        adapters.insert(
            Ecosystem::Pip,
            Arc::new(DelayedAdapter {
                release: events.release.clone(),
            }) as Arc<dyn EcosystemAdapter>,
        );
        let supervisor = WorkerSupervisor::start(SupervisorContext {
            adapters,
            database: None,
            event_sink: events.sink(),
        });
        let first = supervisor
            .submit(WorkerCommand::Update(package_task(Ecosystem::Pip, "a")))
            .unwrap();
        let second = supervisor
            .submit(WorkerCommand::Update(package_task(Ecosystem::Pip, "b")))
            .unwrap();
        events.wait_for_started(first);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!events.has_started(second));
        events.release_first_task();
        events.wait_for_completed(first);
        events.wait_for_started(second);
    }

    #[test]
    fn cancellation_marks_running_task_cancelled_without_retry() {
        let events = TestEvents {
            state: Arc::new((Mutex::new(Vec::new()), std::sync::Condvar::new())),
            release: Arc::new(AtomicBool::new(false)),
        };
        let mut adapters = HashMap::new();
        adapters.insert(
            Ecosystem::Pip,
            Arc::new(DelayedAdapter {
                release: events.release.clone(),
            }) as Arc<dyn EcosystemAdapter>,
        );
        let supervisor = WorkerSupervisor::start(SupervisorContext {
            adapters,
            database: None,
            event_sink: events.sink(),
        });
        let task = supervisor
            .submit(WorkerCommand::Update(package_task(
                Ecosystem::Pip,
                "cancel-me",
            )))
            .unwrap();
        events.wait_for_running(task);
        supervisor.cancel(task).unwrap();
        events.wait_for_status(task, TaskStatus::Cancelled);
        assert!(!events.has_status(task, TaskStatus::Succeeded));
    }
}
