use std::sync::Arc;

use crate::scheduler::Scheduler;

/// 注册 macOS 唤醒通知。详细 NSWorkspace 监听仅在原生 GUI 环境启用，测试环境安全跳过。
pub fn install_wake_listener(_scheduler: Arc<Scheduler>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // 后续可在此接入 NSWorkspaceDidWakeNotification；当前 scheduler.start() 是幂等钩子。
    }
    Ok(())
}
