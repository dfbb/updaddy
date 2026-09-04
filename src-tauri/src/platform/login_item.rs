/// 设置开机启动。非 macOS 平台为无操作桩，保证测试可运行。
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Tauri autostart 插件在应用构建时负责注册；这里保留幂等生命周期钩子。
        let _ = enabled;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = enabled;
    Ok(())
}
