# Task 10 实现报告

## 状态

已完成命令层、事件总线、平台生命周期模块及 worker_flow 非阻塞测试。

## 主要实现

- 新增 `commands.rs`：扫描、更新、卸载、批量更新、取消、状态快照、设置和登录项命令；命令只投递 worker 消息并立即返回任务 ID。
- 新增 `event_bus.rs`：将 worker 事件持久化到 SQLite 并通过 Tauri 事件名称推送；覆盖 `worker-state`、`task-progress`、`package-changed`、`disk-usage`、`log-entry`、`batch-summary`。
- 新增 `platform` 模块：状态栏、登录项和 macOS 唤醒监听生命周期钩子（跨平台安全桩）。
- `lib.rs` 接入数据库、supervisor、autostart 插件、命令注册、关闭窗口隐藏和 scheduler 启动。
- 持久化层新增快照列表查询；补充 `worker_flow` 测试验证更新命令非阻塞。

## 验证

- `cargo check --manifest-path src-tauri/Cargo.toml` 通过（仅已有未使用代码警告）。
- `cargo test --manifest-path src-tauri/Cargo.toml --test worker_flow -- --nocapture` 通过。

## Concerns

- tray 菜单和 NSWorkspace 唤醒通知目前是幂等生命周期桩，待 macOS 原生 UI 集成阶段补充具体菜单项和通知回调。
- 设置暂存于内存，后续可接入现有 `settings` SQLite 表。

## Fix Round 1

- 接入 Tauri tray 菜单（更新数量、批量更新、打开窗口、设置、退出）及默认图标。
- 设置通过 SQLite `settings` 表持久化，增加主题、语言、生态集合和计划时间校验；登录项调用 autostart 插件 enable/disable。
- 状态快照补充完整任务列表，数据库读取错误向命令返回，不再静默吞掉。
- EventBus 覆盖包变更、磁盘占用、批次摘要等事件持久化，并将持久化失败记录为错误日志。
- Worker 发出日志与批次摘要事件；批次提交失败时取消已提交任务；supervisor Drop 取消并等待 worker 线程。
- Scheduler 启动后台调度钩子并提供 catch-up 检查；唤醒监听安装时确保调度器启动。

验证：`cargo test --manifest-path src-tauri/Cargo.toml --lib`（53 项通过）、`cargo check`、`git diff --check`。

## Fix2（2026-09-04）
- Scheduler 增加运行回调、周期状态与定时触发，支持 catch-up。
- 托盘菜单改为固定 id、模板图标，并提供刷新入口与动态更新数量。
- EventBus 磁盘事件使用快照版本，批次摘要写入日志，数据库错误记录。
- 设置保存增加 weekly 计划格式校验。

验证：`cargo test --manifest-path src-tauri/Cargo.toml`（53+ 测试通过）。
