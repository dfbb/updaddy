/// 设置开机启动。非 macOS 平台为无操作桩，保证测试可运行。
pub fn set_enabled<R: tauri::Runtime>(app: &tauri::AppHandle<R>, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    if enabled { app.autolaunch().enable().map_err(|e| e.to_string()) } else { app.autolaunch().disable().map_err(|e| e.to_string()) }
}
