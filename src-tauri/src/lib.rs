pub mod adapters;
pub mod commands;
pub mod core;
pub mod disk_usage;
pub mod event_bus;
pub mod executor;
pub mod persistence;
pub mod platform;
pub mod proxy;
pub mod scheduler;
pub mod workers;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    let database = Some(std::sync::Arc::new(
        crate::persistence::Database::open_default()
            .map_err(|e| tauri::Error::Setup((Box::new(e) as Box<dyn std::error::Error>).into()))?,
    ));
    if let Some(database) = &database {
        let _ = database.mark_active_tasks_interrupted();
        let cutoff = chrono::Utc::now().timestamp() - 90 * 24 * 60 * 60;
        let _ = database.cleanup_before(cutoff);
    }
    let state = crate::commands::AppState::new(database);
    let scheduler_state = state.clone();
    let event_bus = state.event_bus.clone();
    tauri::Builder::default()
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .arg("--autostart")
                .build(),
        )
        .manage(state)
        .setup(move |app| {
            event_bus.attach_app(app.handle().clone());
            crate::platform::tray::setup(app.handle()).map_err(|error| {
                tauri::Error::Setup(
                    (Box::new(std::io::Error::other(error)) as Box<dyn std::error::Error>).into(),
                )
            })?;
            if !std::env::args().any(|argument| argument == "--autostart") {
                if let Some(window) = app.get_webview_window("main") {
                    window.show()?;
                }
            }
            let settings = scheduler_state.settings_snapshot();
            let default_schedule =
                crate::scheduler::Schedule::daily("03:00").expect("valid default schedule");
            let scheduler = std::sync::Arc::new(crate::scheduler::Scheduler::new(default_schedule));
            scheduler.set_schedule(crate::commands::schedule_from_settings(&settings));
            if let Some(database) = &scheduler_state.database {
                if let Ok(Some(raw)) = database.load_scheduler_state("completed_cycles") {
                    match serde_json::from_str(&raw) {
                        Ok(state) => scheduler.restore_state(state),
                        Err(error) => scheduler_state
                            .event_bus
                            .log(format!("计划状态读取失败：{error}"), "scheduler"),
                    }
                }
                let database = database.clone();
                scheduler.set_state_recorder(move |state| {
                    if let Ok(raw) = serde_json::to_string(state) {
                        let _ = database.save_scheduler_state("completed_cycles", &raw);
                    }
                });
            }
            let scheduled_state = scheduler_state.clone();
            scheduler.set_runner(move || {
                if crate::commands::schedule_from_settings(&scheduled_state.settings_snapshot())
                    .is_none()
                {
                    return false;
                }
                match crate::commands::submit_all_visible(&scheduled_state) {
                    Ok(_) => true,
                    Err(error) => {
                        scheduled_state
                            .event_bus
                            .log(format!("计划更新等待重试：{error}"), "scheduler");
                        false
                    }
                }
            });
            crate::platform::macos::install_wake_listener(scheduler.clone()).map_err(|error| {
                tauri::Error::Setup(
                    (Box::new(std::io::Error::other(error)) as Box<dyn std::error::Error>).into(),
                )
            })?;
            // Keep the scheduler handle in shared application state so settings changes take
            // effect without restarting the desktop process.
            scheduler_state.set_scheduler(scheduler);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::scan_ecosystem,
            commands::scan_all_visible,
            commands::update_package,
            commands::uninstall_package,
            commands::update_all_visible,
            commands::update_ecosystem,
            commands::cancel_task,
            commands::get_state_snapshot,
            commands::get_settings,
            commands::save_settings,
            commands::set_login_item,
            commands::open_settings,
            commands::refresh_disk_usage,
            commands::cleanup_expired_logs,
            commands::list_task_attempts,
            commands::list_task_logs,
            commands::retry_task,
        ])
        .build(tauri::generate_context!())?
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let state = app.state::<crate::commands::AppState>();
                if !state.is_shutdown_started() {
                    api.prevent_exit();
                    state.shutdown();
                    app.exit(0);
                }
            }
        });
    Ok(())
}
