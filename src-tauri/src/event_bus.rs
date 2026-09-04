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
            let result = match &event {
                WorkerEvent::WorkerState {
                    ecosystem,
                    state,
                    emitted_at,
                    ..
                } => {
                    database.set_worker_state(*ecosystem, state, *emitted_at)
                }
                WorkerEvent::TaskProgress {
                    task_id,
                    status,
                    error,
                    ..
                } => {
                    database.update_task(*task_id, *status, *error).and_then(|ok| if ok { Ok(()) } else { Err(crate::persistence::PersistenceError::InvalidValue("task missing".into())) })
                }
                WorkerEvent::LogEntry { entry, .. } => {
                    database.append_log(entry)
                }
                WorkerEvent::PackageChanged { package, .. } => database.save_snapshot(package),
                WorkerEvent::DiskUsage { ecosystem, package_id, disk_usage, .. } => {
                    let entry = crate::persistence::DiskUsageCacheEntry { ecosystem: format!("{ecosystem:?}"), package_id: package_id.clone(), installed_version: String::new(), install_root: String::new(), path_signature: String::new(), bytes: disk_usage.bytes, status: disk_usage.status, scanned_at: chrono::Utc::now().timestamp() };
                    database.upsert_disk_usage(&entry)
                }
                WorkerEvent::BatchSummary { batch_id, ecosystem, total, .. } => database.save_scheduler_state(&format!("batch:{batch_id}"), &format!("{ecosystem:?}:{total}")),
            };
            if let Err(err) = result { let _ = database.append_log(&LogEntry { message: format!("event persistence failed: {err}"), emitted_at: chrono::Utc::now().timestamp(), stream: "error".into() }); }
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
