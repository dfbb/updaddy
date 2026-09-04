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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    let database = crate::persistence::Database::open_default()
        .ok()
        .map(std::sync::Arc::new);
    let state = crate::commands::AppState::new(database);
    let event_bus = state.event_bus.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .manage(state)
        .setup(move |app| {
            event_bus.attach_app(app.handle().clone());
            let _ = crate::platform::tray::setup(&app.handle());
            let scheduler = std::sync::Arc::new(crate::scheduler::Scheduler::new(
                crate::scheduler::Schedule::daily("03:00").expect("valid default schedule"),
            ));
            scheduler.start();
            let _ = crate::platform::macos::install_wake_listener(scheduler);
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
        .run(tauri::generate_context!())
}
