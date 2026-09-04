use tauri::{AppHandle, Manager, Emitter, menu::{MenuBuilder, MenuItem, PredefinedMenuItem}, tray::TrayIconBuilder};

/// 创建状态栏菜单。菜单动作通过 commands 复用同一 supervisor，避免重复业务逻辑。
pub fn setup(app: &AppHandle) -> Result<(), String> {
    let total = app.state::<crate::commands::AppState>().database.as_ref().and_then(|db| db.list_snapshots().ok()).map(|items| items.into_iter().filter(|p| p.update_available && app.state::<crate::commands::AppState>().visible(p.ecosystem)).count()).unwrap_or(0);
    let count = MenuItem::with_id(app, "count", format!("可更新：{total}"), true, None::<&str>).map_err(|e| e.to_string())?;
    let update = MenuItem::with_id(app, "update", "一键更新", true, None::<&str>).map_err(|e| e.to_string())?;
    let open = MenuItem::with_id(app, "open", "打开主窗口", true, None::<&str>).map_err(|e| e.to_string())?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>).map_err(|e| e.to_string())?;
    let quit = PredefinedMenuItem::quit(app, Some("退出")).map_err(|e| e.to_string())?;
    let menu = MenuBuilder::new(app).items(&[&count, &update, &open, &settings, &quit]).build().map_err(|e| e.to_string())?;
    let mut tray = TrayIconBuilder::new().menu(&menu).tooltip("updaddy");
    if let Some(icon) = app.default_window_icon() { tray = tray.icon(icon.clone()).icon_as_template(true); }
    tray.on_menu_event(|app, event| match event.id().as_ref() { "open"|"count" => { if let Some(w)=app.get_webview_window("main") { let _=w.show(); let _=w.set_focus(); } }, "settings" => { if let Some(w)=app.get_webview_window("main") { let _=w.show(); let _=w.set_focus(); let _=w.emit("open-settings", ()); } }, "update" => { let _=app.emit("tray-update-all", ()); }, _=>{} }).build(app).map_err(|e| e.to_string())?;
    Ok(())
}
