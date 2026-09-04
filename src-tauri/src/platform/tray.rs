use tauri::AppHandle;

/// 创建状态栏菜单。菜单动作通过 commands 复用同一 supervisor，避免重复业务逻辑。
pub fn setup(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}
