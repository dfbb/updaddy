use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::{Ecosystem, Operation, PackageTask};
use crate::event_bus::EventBus;
use crate::persistence::Database;
use crate::workers::{
    SupervisorContext, WorkerCommand, WorkerEvent, WorkerEventSink, WorkerSupervisor,
};

pub type TaskId = Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageId {
    pub ecosystem: Ecosystem,
    pub name: String,
}

impl PackageId {
    pub fn new(ecosystem: Ecosystem, name: impl Into<String>) -> Self {
        Self {
            ecosystem,
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub visible_ecosystems: HashSet<Ecosystem>,
    pub theme: String,
    pub locale: String,
    pub schedule: Option<String>,
    pub login_item: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            visible_ecosystems: Ecosystem::ALL.into_iter().collect(),
            theme: "system".into(),
            locale: "en".into(),
            schedule: None,
            login_item: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StateSnapshot {
    pub workers: HashMap<Ecosystem, String>,
    pub packages: Vec<crate::core::PackageRecord>,
    pub logs: Vec<crate::core::LogEntry>,
    pub active_tasks: usize,
    pub tasks: Vec<PackageTask>,
}

#[derive(Clone)]
pub struct AppState {
    pub supervisor: Arc<WorkerSupervisor>,
    pub database: Option<Arc<Database>>,
    pub event_bus: EventBus,
    settings: Arc<Mutex<Settings>>,
    pub(crate) test_events: Option<Arc<Mutex<Vec<WorkerEvent>>>>,
    shutdown_started: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(database: Option<Arc<Database>>) -> Self {
        let bus = EventBus::new(database.clone());
        let supervisor = WorkerSupervisor::start(SupervisorContext {
            adapters: crate::adapters::default_adapters(),
            database: database.clone(),
            event_sink: bus.sink(),
            executor: crate::adapters::ExecutorContext::new(),
        });
        let settings = match database.as_ref() {
            Some(db) => match db.load_setting("settings") {
                Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_else(|error| {
                    eprintln!("updaddy: invalid persisted settings: {error}");
                    Settings::default()
                }),
                Ok(None) => Settings::default(),
                Err(error) => {
                    eprintln!("updaddy: failed to load settings: {error}");
                    Settings::default()
                }
            },
            None => Settings::default(),
        };
        Self {
            supervisor: Arc::new(supervisor),
            database,
            event_bus: bus,
            settings: Arc::new(Mutex::new(settings)),
            test_events: None,
            shutdown_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn visible(&self, ecosystem: Ecosystem) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .visible_ecosystems
            .contains(&ecosystem)
    }
    pub fn worker_events(&self) -> Vec<WorkerEvent> {
        self.test_events
            .as_ref()
            .map(|e| e.lock().unwrap().clone())
            .unwrap_or_default()
    }

    /// 为命令非阻塞测试提供永不完成的 worker。
    pub fn test_with_blocked_worker() -> Self {
        struct Blocked;
        #[async_trait::async_trait]
        impl crate::workers::EcosystemAdapter for Blocked {
            fn ecosystem(&self) -> Ecosystem {
                Ecosystem::Npm
            }
            async fn execute(
                &self,
                _c: &crate::adapters::ExecutorContext,
                _t: &PackageTask,
                cancel: CancellationToken,
            ) -> Result<(), crate::core::TaskErrorKind> {
                cancel.cancelled().await;
                Err(crate::core::TaskErrorKind::CommandFailed)
            }
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_sink = events.clone();
        let sink: WorkerEventSink = Arc::new(move |event| events_sink.lock().unwrap().push(event));
        let mut context = SupervisorContext::new(sink);
        context.adapters.insert(Ecosystem::Npm, Arc::new(Blocked));
        let supervisor = WorkerSupervisor::start(context);
        events.lock().unwrap().clear();
        Self {
            supervisor: Arc::new(supervisor),
            database: None,
            event_bus: EventBus::new(None),
            settings: Arc::new(Mutex::new(Settings::default())),
            test_events: Some(events),
            shutdown_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn shutdown(&self) {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        self.supervisor.shutdown();
    }

    pub fn is_shutdown_started(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire)
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// 将持久化设置转换为 scheduler 使用的强类型计划。
pub fn schedule_from_settings(settings: &Settings) -> Option<crate::scheduler::Schedule> {
    let raw = settings.schedule.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some(value) = raw.strip_prefix("weekly:") {
        let (weekday, time) = value.split_once(' ')?;
        return crate::scheduler::Schedule::weekly(weekday, time).ok();
    }
    if let Some((weekday, time)) = raw.split_once(' ') {
        return crate::scheduler::Schedule::weekly(weekday, time).ok();
    }
    crate::scheduler::Schedule::daily(raw).ok()
}

fn ensure_visible(app: &AppState, ecosystem: Ecosystem) -> Result<(), String> {
    if app.visible(ecosystem) {
        Ok(())
    } else {
        Err("生态已隐藏".into())
    }
}

pub fn submit_update_command(app: &AppState, package: PackageId) -> Result<TaskId, String> {
    ensure_visible(app, package.ecosystem)?;
    let task = PackageTask::new(package.ecosystem, package.name, Operation::Update);
    app.supervisor
        .submit(WorkerCommand::Update(task))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn scan_ecosystem(app: State<'_, AppState>, ecosystem: Ecosystem) -> Result<TaskId, String> {
    ensure_visible(&app, ecosystem)?;
    app.supervisor
        .submit(WorkerCommand::Scan(ecosystem))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn scan_all_visible(app: State<'_, AppState>) -> Result<Vec<TaskId>, String> {
    let commands = Ecosystem::ALL
        .into_iter()
        .filter(|e| app.visible(*e))
        .map(WorkerCommand::Scan)
        .collect();
    app.supervisor
        .submit_batch(commands)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_package(app: State<'_, AppState>, package: PackageId) -> Result<TaskId, String> {
    submit_update_command(&app, package)
}

#[tauri::command]
pub fn uninstall_package(app: State<'_, AppState>, package: PackageId) -> Result<TaskId, String> {
    ensure_visible(&app, package.ecosystem)?;
    let task = PackageTask::new(package.ecosystem, package.name, Operation::Uninstall);
    app.supervisor
        .submit(WorkerCommand::Uninstall(task))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_all_visible(app: State<'_, AppState>) -> Result<Vec<TaskId>, String> {
    submit_all_visible(&app)
}

pub fn submit_all_visible(app: &AppState) -> Result<Vec<TaskId>, String> {
    let snapshots = app
        .database
        .as_ref()
        .map(|db| db.list_snapshots().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    let visible = crate::scheduler::EnabledEcosystems::only(
        &Ecosystem::ALL
            .into_iter()
            .filter(|e| app.visible(*e))
            .collect::<Vec<_>>(),
    );
    let commands = crate::scheduler::Scheduler::plan_visible_updates(visible, snapshots)
        .into_iter()
        .map(WorkerCommand::Update)
        .collect();
    app.supervisor
        .submit_batch(commands)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn cancel_task(app: State<'_, AppState>, task_id: TaskId) -> Result<(), String> {
    app.supervisor.cancel(task_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_state_snapshot(app: State<'_, AppState>) -> Result<StateSnapshot, String> {
    let mut workers = HashMap::new();
    if let Some(db) = &app.database {
        for ecosystem in Ecosystem::ALL {
            if let Some((state, _)) = db.worker_state(ecosystem).map_err(|e| e.to_string())? {
                workers.insert(ecosystem, state);
            }
        }
    }
    let packages = app
        .database
        .as_ref()
        .map(|db| db.list_snapshots().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    let logs = app
        .database
        .as_ref()
        .map(|db| db.list_logs().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    let tasks = app
        .database
        .as_ref()
        .map(|db| db.list_tasks().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    Ok(StateSnapshot {
        workers,
        packages,
        logs,
        active_tasks: app.supervisor.active_task_count(),
        tasks,
    })
}

#[tauri::command]
pub fn get_settings(app: State<'_, AppState>) -> Result<Settings, String> {
    if let Some(db) = &app.database {
        if let Some(raw) = db.load_setting("settings").map_err(|e| e.to_string())? {
            if let Ok(s) = serde_json::from_str(&raw) {
                *app.settings.lock().unwrap_or_else(|p| p.into_inner()) = s;
            }
        }
    }
    Ok(app
        .settings
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone())
}

#[tauri::command]
pub fn save_settings(app: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        return Err("主题无效".into());
    }
    if settings.locale.trim().is_empty() || settings.visible_ecosystems.is_empty() {
        return Err("设置无效".into());
    }
    if let Some(schedule) = &settings.schedule {
        if !schedule.is_empty() {
            let valid = if let Some((day, time)) = schedule
                .strip_prefix("weekly:")
                .and_then(|s| s.split_once(' '))
            {
                crate::scheduler::Schedule::weekly(day, time).is_ok()
            } else if let Some((day, time)) = schedule.split_once(' ') {
                crate::scheduler::Schedule::weekly(day, time).is_ok()
            } else {
                crate::scheduler::Schedule::daily(schedule).is_ok()
            };
            if !valid {
                return Err("计划时间无效".into());
            }
        }
    }
    if let Some(db) = &app.database {
        db.save_setting(
            "settings",
            &serde_json::to_string(&settings).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    *app.settings.lock().unwrap_or_else(|p| p.into_inner()) = settings;
    Ok(())
}

#[tauri::command]
pub fn set_login_item(
    app: State<'_, AppState>,
    handle: tauri::AppHandle,
    enabled: bool,
) -> Result<(), String> {
    crate::platform::login_item::set_enabled(&handle, enabled)?;
    if let Some(db) = &app.database {
        let mut settings = app
            .settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        settings.login_item = enabled;
        db.save_setting(
            "settings",
            &serde_json::to_string(&settings).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    app.settings
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .login_item = enabled;
    Ok(())
}

#[tauri::command]
pub fn open_settings(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        let _ = window.emit("open-settings", ());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn package_id(ecosystem: &str, name: &str) -> PackageId {
        let ecosystem = match ecosystem {
            "homebrew" => Ecosystem::Homebrew,
            "npm" => Ecosystem::Npm,
            "pip" => Ecosystem::Pip,
            "gem" => Ecosystem::Gem,
            "rustup" => Ecosystem::Rustup,
            _ => panic!("unknown ecosystem"),
        };
        PackageId::new(ecosystem, name)
    }
}
