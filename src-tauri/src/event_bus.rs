use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter};

use crate::core::LogEntry;
use crate::persistence::Database;
use crate::workers::{WorkerEvent, WorkerEventSink};

/// Worker 事件的持久化和前端推送桥接。没有 AppHandle 时仍会记录事件，便于测试。
#[derive(Clone)]
pub struct EventBus {
    database: Option<Arc<Database>>,
    app: Arc<Mutex<Option<AppHandle>>>,
}

impl EventBus {
    pub fn new(database: Option<Arc<Database>>) -> Self {
        Self {
            database,
            app: Arc::new(Mutex::new(None)),
        }
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.app.lock().unwrap_or_else(|p| p.into_inner()) = Some(app);
    }

    pub fn sink(&self) -> WorkerEventSink {
        let bus = self.clone();
        Arc::new(move |event| bus.publish(event))
    }

    pub fn publish(&self, event: WorkerEvent) {
        if let Some(database) = &self.database {
            match &event {
                WorkerEvent::WorkerState {
                    ecosystem,
                    state,
                    emitted_at,
                    ..
                } => {
                    let _ = database.set_worker_state(*ecosystem, state, *emitted_at);
                }
                WorkerEvent::TaskProgress {
                    task_id,
                    status,
                    error,
                    ..
                } => {
                    let _ = database.update_task(*task_id, *status, *error);
                }
                WorkerEvent::LogEntry { entry, .. } => {
                    let _ = database.append_log(entry);
                }
                _ => {}
            }
        }
        let name = match &event {
            WorkerEvent::WorkerState { .. } => "worker-state",
            WorkerEvent::TaskProgress { .. } => "task-progress",
            WorkerEvent::PackageChanged { .. } => "package-changed",
            WorkerEvent::DiskUsage { .. } => "disk-usage",
            WorkerEvent::LogEntry { .. } => "log-entry",
            WorkerEvent::BatchSummary { .. } => "batch-summary",
        };
        if let Some(app) = self.app.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = app.emit(name, &event);
        }
    }

    pub fn log(&self, message: impl Into<String>, stream: impl Into<String>) {
        let entry = LogEntry {
            message: message.into(),
            emitted_at: chrono::Utc::now().timestamp(),
            stream: stream.into(),
        };
        if let Some(database) = &self.database {
            let _ = database.append_log(&entry);
        }
        if let Some(app) = self.app.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = app.emit("log-entry", &entry);
        }
    }
}
