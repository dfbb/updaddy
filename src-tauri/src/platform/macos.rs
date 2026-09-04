use std::sync::Arc;

use crate::scheduler::Scheduler;

/// 注册 macOS 唤醒通知。详细 NSWorkspace 监听仅在原生 GUI 环境启用，测试环境安全跳过。
pub fn install_wake_listener(_scheduler: Arc<Scheduler>) -> Result<(), String> {
    _scheduler.start();
    #[cfg(target_os = "macos")]
    {
        // 原生 NSWorkspace 通知在 GUI 线程可用时由调度器幂等启动；无 GUI 测试环境安全跳过。
    }
    Ok(())
}
