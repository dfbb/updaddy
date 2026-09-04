# Task 9 实现报告

## 完成内容

- 新增 `scheduler` 模块，实现每日/每周计划解析与下一次 due 时间计算。
- 新增 `SchedulerState` 与 `CatchUp::should_run`，同一周期只补执行一次。
- 新增 `EnabledEcosystems::only` 和计划更新任务过滤，隐藏生态不会进入计划批次。
- supervisor 增加批次投递互斥入口（`submit_batch`）及活动状态查询。
- persistence 增加 scheduler_state 键值读写方法。

## 验证

运行：

```text
cargo test --manifest-path src-tauri/Cargo.toml scheduler:: -- --nocapture
```

结果：3 个 scheduler 测试通过。

## Concerns

- 当前 `Scheduler::start()` 是无阻塞启动钩子，实际应用的定时器/系统唤醒事件接入留给后续 Tauri 平台任务。
- 计划状态持久化提供通用键值 API；应用层需约定配置版本与周期 ID 的键格式。
