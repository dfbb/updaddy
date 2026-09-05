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
    let state = crate::commands::AppState::new(database);
    let scheduler_state = state.clone();
    let event_bus = state.event_bus.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .manage(state)
        .setup(move |app| {
            event_bus.attach_app(app.handle().clone());
            crate::platform::tray::setup(&app.handle()).map_err(|error| {
                tauri::Error::Setup(
                    (Box::new(std::io::Error::other(error)) as Box<dyn std::error::Error>).into(),
                )
            })?;
            let settings = scheduler_state.settings_snapshot();
            let schedule =
                crate::commands::schedule_from_settings(&settings).unwrap_or_else(|| {
                    crate::scheduler::Schedule::daily("03:00").expect("valid default schedule")
                });
            let scheduler = std::sync::Arc::new(crate::scheduler::Scheduler::new(schedule));
            let scheduled_state = scheduler_state.clone();
            scheduler.set_runner(move || {
                if let Err(error) = crate::commands::submit_all_visible(&scheduled_state) {
                    scheduled_state
                        .event_bus
                        .log(format!("计划更新失败：{error}"), "scheduler");
                }
            });
            crate::platform::macos::install_wake_listener(scheduler).map_err(|error| {
                tauri::Error::Setup(
                    (Box::new(std::io::Error::other(error)) as Box<dyn std::error::Error>).into(),
                )
            })?;
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
            commands::cancel_task,
            commands::get_state_snapshot,
            commands::get_settings,
            commands::save_settings,
            commands::set_login_item,
            commands::open_settings,
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
