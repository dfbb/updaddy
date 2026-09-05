use std::sync::Arc;

use crate::scheduler::Scheduler;

/// 注册 macOS 唤醒通知。详细 NSWorkspace 监听仅在原生 GUI 环境启用，测试环境安全跳过。
pub fn install_wake_listener(_scheduler: Arc<Scheduler>) -> Result<(), String> {
    _scheduler.start();
    #[cfg(target_os = "macos")]
    {
        use block2::RcBlock;
        use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
        use objc2_foundation::{NSNotification, NSOperationQueue};
        use std::ptr::NonNull;

        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let scheduler = _scheduler.clone();
        let callback = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            scheduler.trigger();
        });
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                Some(&NSOperationQueue::mainQueue()),
                &callback,
            )
        };
        // NSNotificationCenter 持有 token；泄漏此 token 直到应用退出，
        // 避免回调在 scheduler 生命周期内被提前释放。
        std::mem::forget(observer);
    }
    Ok(())
}
