use crate::core::Ecosystem;
use tauri::{
    menu::{MenuBuilder, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

/// 创建状态栏菜单。菜单动作通过 commands 复用同一 supervisor，避免重复业务逻辑。
pub fn setup(app: &AppHandle) -> Result<(), String> {
    let menu = build_menu(app).map_err(|e| e.to_string())?;
    let mut tray = TrayIconBuilder::with_id("updaddy")
        .menu(&menu)
        .tooltip("updaddy");
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone()).icon_as_template(true);
    }
    tray.on_menu_event(|app, event| match event.id().as_ref() {
        "open" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        "count" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
                let _ = w.emit("open-overview", ());
            }
        }
        "settings" => {
            let _ = crate::commands::open_settings(app.clone());
        }
        "update" => {
            let state = app.state::<crate::commands::AppState>();
            if let Err(error) = crate::commands::submit_all_visible(&state) {
                state
                    .event_bus
                    .log(format!("状态栏一键更新失败：{error}"), "tray");
            }
        }
        _ => {}
    })
    .build(app)
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn build_menu(app: &AppHandle) -> Result<tauri::menu::Menu<tauri::Wry>, String> {
    let state = app.state::<crate::commands::AppState>();
    let chinese = state
        .settings_snapshot()
        .locale
        .to_ascii_lowercase()
        .starts_with("zh");
    let snapshots = state
        .database
        .as_ref()
        .map(|db| db.list_snapshots().map_err(|e| e.to_string()))
        .transpose()?
        .unwrap_or_default();
    let total = snapshots
        .into_iter()
        .filter(|p| p.update_available && state.visible(p.ecosystem))
        .count();
    let checking = if let Some(db) = &state.database {
        let mut checking = false;
        for ecosystem in Ecosystem::ALL {
            if let Some((worker_state, _)) = db
                .worker_state(ecosystem)
                .map_err(|error| error.to_string())?
            {
                checking |= worker_state == "running";
            }
        }
        checking
    } else {
        false
    };
    let label = if checking {
        if chinese {
            "正在检查更新…"
        } else {
            "Checking for updates…"
        }
        .to_owned()
    } else if total == 0 {
        if chinese {
            "已是最新"
        } else {
            "Up to date"
        }
        .to_owned()
    } else {
        if chinese {
            format!("可更新：{total}")
        } else {
            format!("Updates available: {total}")
        }
    };
    let count =
        MenuItem::with_id(app, "count", label, true, None::<&str>).map_err(|e| e.to_string())?;
    let update = MenuItem::with_id(
        app,
        "update",
        if chinese {
            "一键更新"
        } else {
            "Update all"
        },
        state.supervisor.active_task_count() == 0,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let open = MenuItem::with_id(
        app,
        "open",
        if chinese {
            "打开主窗口"
        } else {
            "Open main window"
        },
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let settings = MenuItem::with_id(
        app,
        "settings",
        if chinese { "设置" } else { "Settings" },
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let quit = PredefinedMenuItem::quit(app, Some(if chinese { "退出" } else { "Quit" }))
        .map_err(|e| e.to_string())?;
    MenuBuilder::new(app)
        .items(&[&count, &update, &open, &settings, &quit])
        .build()
        .map_err(|e| e.to_string())
}

pub fn refresh(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id("updaddy") {
        match build_menu(app) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
            }
            Err(error) => eprintln!("updaddy tray refresh failed: {error}"),
        }
    }
}
