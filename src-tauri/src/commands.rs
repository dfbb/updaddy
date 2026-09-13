use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::{Ecosystem, Operation, PackageTask, TaskStatus};
use crate::event_bus::EventBus;
use crate::persistence::Database;
use crate::proxy::ProxyConfig;
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxySettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSettings {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub retain_command_output: bool,
}

fn default_log_level() -> String {
    "info".into()
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            retain_command_output: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_visible_ecosystems")]
    pub visible_ecosystems: HashSet<Ecosystem>,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_locale")]
    pub locale: String,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default)]
    pub login_item: bool,
    #[serde(default)]
    pub proxy: ProxySettings,
    #[serde(default)]
    pub logs: LogSettings,
}

fn default_visible_ecosystems() -> HashSet<Ecosystem> {
    Ecosystem::ALL.into_iter().collect()
}

fn default_theme() -> String {
    "system".into()
}

fn default_locale() -> String {
    "en".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            visible_ecosystems: default_visible_ecosystems(),
            theme: default_theme(),
            locale: default_locale(),
            schedule: None,
            login_item: false,
            proxy: ProxySettings::default(),
            logs: LogSettings::default(),
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
    pub batches: Vec<crate::core::OperationBatch>,
}

#[derive(Clone)]
pub struct AppState {
    pub supervisor: Arc<WorkerSupervisor>,
    pub database: Option<Arc<Database>>,
    pub event_bus: EventBus,
    executor: crate::adapters::ExecutorContext,
    scheduler: Arc<Mutex<Option<Arc<crate::scheduler::Scheduler>>>>,
    settings: Arc<Mutex<Settings>>,
    pub(crate) test_events: Option<Arc<Mutex<Vec<WorkerEvent>>>>,
    shutdown_started: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(database: Option<Arc<Database>>) -> Self {
        let settings = Arc::new(Mutex::new(load_settings(database.as_ref())));
        let bus = EventBus::new(database.clone());
        let output_settings = settings.clone();
        let output_bus = bus.clone();
        let executor =
            crate::adapters::ExecutorContext::with_output_sink(Arc::new(move |output| {
                let settings = output_settings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let retain = settings.logs.retain_command_output
                    && (settings.logs.level == "debug"
                        || settings.logs.level == "info"
                        || output.stream == "stderr");
                let retain_task_error = output.task_id.is_some()
                    && matches!(output.stream.as_str(), "stderr" | "error");
                if output.task_id.is_some() || retain {
                    let entry = crate::core::LogEntry {
                        message: output.text,
                        emitted_at: output.emitted_at,
                        stream: output.stream,
                    };
                    if let Some(task_id) = output.task_id {
                        output_bus.task_output(task_id, entry, retain || retain_task_error);
                    } else {
                        output_bus.log(entry.message, entry.stream);
                    }
                }
            }));
        if let Ok(config) = proxy_config_from_settings(&settings.lock().unwrap().proxy) {
            executor.set_proxy_config(config);
        }
        let supervisor = WorkerSupervisor::start(SupervisorContext {
            adapters: crate::adapters::default_adapters(),
            database: database.clone(),
            event_sink: bus.sink(),
            executor: executor.clone(),
            detect_on_start: true,
        });
        Self {
            supervisor: Arc::new(supervisor),
            database,
            event_bus: bus,
            executor,
            scheduler: Arc::new(Mutex::new(None)),
            settings,
            test_events: None,
            shutdown_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn visible(&self, ecosystem: Ecosystem) -> bool {
        let configured = self
            .settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .visible_ecosystems
            .contains(&ecosystem);
        if !configured {
            return false;
        }
        self.database
            .as_ref()
            .and_then(|database| database.worker_state(ecosystem).ok().flatten())
            .is_none_or(|(state, _)| !matches!(state.as_str(), "missing" | "disabled"))
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
            executor: crate::adapters::ExecutorContext::new(),
            scheduler: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(Settings::default())),
            test_events: Some(events),
            shutdown_started: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn shutdown(&self) {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(scheduler) = self
            .scheduler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            scheduler.stop();
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

    pub fn set_scheduler(&self, scheduler: Arc<crate::scheduler::Scheduler>) {
        *self
            .scheduler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(scheduler);
    }

    pub fn update_scheduler(&self, schedule: Option<crate::scheduler::Schedule>) {
        if let Some(scheduler) = self
            .scheduler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            scheduler.set_schedule(schedule);
        }
    }
}

fn load_settings(database: Option<&Arc<Database>>) -> Settings {
    match database {
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
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn old_settings_payload_gets_new_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"visible_ecosystems":["npm"],"theme":"dark","locale":"zh-CN","schedule":null,"login_item":false}"#,
        )
        .unwrap();
        assert!(!settings.proxy.enabled);
        assert_eq!(settings.logs.level, "info");
    }

    #[test]
    fn proxy_settings_require_address_when_enabled() {
        let error = proxy_config_from_settings(&ProxySettings {
            enabled: true,
            ..ProxySettings::default()
        })
        .unwrap_err();
        assert!(error.contains("不能为空"));
    }
}

fn proxy_config_from_settings(settings: &ProxySettings) -> Result<Option<ProxyConfig>, String> {
    if !settings.enabled {
        return Ok(None);
    }
    let address = settings.address.trim();
    if address.is_empty() {
        return Err("代理地址不能为空".into());
    }
    let normalized = if address.contains("://") {
        address.to_owned()
    } else {
        format!("socks5://{address}")
    };
    let mut config = ProxyConfig::parse(&normalized).map_err(|error| error.to_string())?;
    config.username = settings
        .username
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or(config.username);
    config.password = settings
        .password
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or(config.password);
    if config.password.is_some() && config.username.is_none() {
        return Err("代理密码必须与用户名一起提供".into());
    }
    Ok(Some(config))
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

#[tauri::command]
pub fn update_ecosystem(
    app: State<'_, AppState>,
    ecosystem: Ecosystem,
) -> Result<Vec<TaskId>, String> {
    ensure_visible(&app, ecosystem)?;
    submit_visible_updates(
        &app,
        crate::scheduler::EnabledEcosystems::only(&[ecosystem]),
    )
}

pub fn submit_all_visible(app: &AppState) -> Result<Vec<TaskId>, String> {
    let visible = crate::scheduler::EnabledEcosystems::only(
        &Ecosystem::ALL
            .into_iter()
            .filter(|e| app.visible(*e))
            .collect::<Vec<_>>(),
    );
    submit_visible_updates(app, visible)
}

fn submit_visible_updates(
    app: &AppState,
    visible: crate::scheduler::EnabledEcosystems,
) -> Result<Vec<TaskId>, String> {
    let snapshots = app
        .database
        .as_ref()
        .map(|db| db.list_snapshots().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
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
    let batches = app
        .database
        .as_ref()
        .map(|db| db.list_batches().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    Ok(StateSnapshot {
        workers,
        packages,
        logs,
        active_tasks: app.supervisor.active_task_count(),
        tasks,
        batches,
    })
}

#[tauri::command]
pub fn get_settings(app: State<'_, AppState>) -> Result<Settings, String> {
    if let Some(db) = &app.database {
        if let Some(raw) = db.load_setting("settings").map_err(|e| e.to_string())? {
            if let Ok(s) = serde_json::from_str::<Settings>(&raw) {
                if let Ok(config) = proxy_config_from_settings(&s.proxy) {
                    app.executor.set_proxy_config(config);
                }
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
pub fn save_settings(
    app: State<'_, AppState>,
    handle: tauri::AppHandle,
    settings: Settings,
) -> Result<(), String> {
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        return Err("主题无效".into());
    }
    if settings.locale.trim().is_empty() || settings.visible_ecosystems.is_empty() {
        return Err("设置无效".into());
    }
    if !matches!(
        settings.logs.level.as_str(),
        "error" | "warn" | "info" | "debug"
    ) {
        return Err("日志级别无效".into());
    }
    let proxy = proxy_config_from_settings(&settings.proxy)?;
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
    app.executor.set_proxy_config(proxy);
    let next_schedule = schedule_from_settings(&settings);
    *app.settings.lock().unwrap_or_else(|p| p.into_inner()) = settings;
    app.update_scheduler(next_schedule);
    crate::platform::tray::refresh(&handle);
    Ok(())
}

#[tauri::command]
pub fn refresh_disk_usage(
    app: State<'_, AppState>,
    package: crate::core::PackageRecord,
) -> Result<TaskId, String> {
    ensure_visible(&app, package.ecosystem)?;
    app.supervisor
        .submit(WorkerCommand::RefreshDiskUsage(package))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cleanup_expired_logs(app: State<'_, AppState>) -> Result<u64, String> {
    let Some(database) = &app.database else {
        return Ok(0);
    };
    let cutoff = chrono::Utc::now().timestamp() - 90 * 24 * 60 * 60;
    database
        .cleanup_before(cutoff)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_task_attempts(
    app: State<'_, AppState>,
    task_id: TaskId,
) -> Result<Vec<crate::persistence::TaskAttempt>, String> {
    app.database
        .as_ref()
        .map(|database| {
            database
                .list_task_attempts(task_id)
                .map_err(|error| error.to_string())
        })
        .transpose()
        .map(|value| value.unwrap_or_default())
}

#[tauri::command]
pub fn list_task_logs(
    app: State<'_, AppState>,
    task_id: TaskId,
) -> Result<Vec<crate::core::LogEntry>, String> {
    app.database
        .as_ref()
        .map(|database| {
            database
                .list_task_logs(task_id)
                .map_err(|error| error.to_string())
        })
        .transpose()
        .map(|value| value.unwrap_or_default())
}

#[tauri::command]
pub fn retry_task(app: State<'_, AppState>, task_id: TaskId) -> Result<TaskId, String> {
    let Some(database) = &app.database else {
        return Err("历史任务不可用".into());
    };
    let task = database
        .list_tasks()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|task| task.task_id == task_id)
        .ok_or_else(|| "任务不存在".to_owned())?;
    if !matches!(task.status, TaskStatus::Failed | TaskStatus::Interrupted) {
        return Err("只有失败或中断任务可以重试".into());
    }
    ensure_visible(&app, task.ecosystem)?;
    let command = match task.operation {
        Operation::Update => WorkerCommand::Retry(PackageTask::new(
            task.ecosystem,
            task.name,
            Operation::Update,
        )),
        Operation::Uninstall => WorkerCommand::Retry(PackageTask::new(
            task.ecosystem,
            task.name,
            Operation::Uninstall,
        )),
        _ => return Err("该任务类型不支持重试".into()),
    };
    app.supervisor
        .submit(command)
        .map_err(|error| error.to_string())
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
