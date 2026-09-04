# Task 3 报告：SQLite 持久化和磁盘缓存表

## 改动

- 新增 `src-tauri/migrations/0001_initial.sql`，创建设置、worker、管理器配置、快照、磁盘缓存、批次、任务、尝试、日志和 scheduler 状态表，并添加常用索引。
- 新增 `src-tauri/src/persistence/{mod.rs,db.rs,migrations.rs,repositories.rs,cleanup.rs}`。
- `Database::open` 创建父目录、设置 Unix 权限（目录 `0700`、数据库文件 `0600`），启用 SQLite WAL 和外键约束；所有写入在 `Mutex<Connection>` 保护下使用事务。
- 实现快照保存、磁盘用量缓存 upsert/查询、批次及任务写入、日志追加、worker 状态写入、运行中任务原子标记为 `interrupted`、按 cutoff 清理历史批次/任务/尝试/日志。
- 为核心 `LogEntry` 增加 `LogEntry::at` 测试构造器，为持久化层增加 `DiskUsageCacheEntry::ready`。

## 测试与检查

- `cargo fmt --manifest-path src-tauri/Cargo.toml`：通过。
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`：通过（格式化后静态检查通过）。
- `cargo test --manifest-path src-tauri/Cargo.toml persistence:: -- --nocapture`：未能启动。当前 registry 仅提供 `objc2-app-kit` 0.3/0.2 版本，无法解析 Cargo.toml 中要求的 `^0.6`；`--offline` 也因同一版本缺失失败。

## Concerns

- 由于依赖解析阻塞，无法在本环境完成 Rust 编译和运行时 SQL 测试；依赖可用后应优先重跑指定测试。
- 清理逻辑不会删除磁盘缓存行，以避免误删仍被快照或其他引用使用的最新缓存；后续若建立明确引用关系，可增加基于引用的过期缓存回收。

## Fix round 1

- `TaskStatus` 增加 `Interrupted`，与启动恢复逻辑的持久化状态一致，并增加序列化回归断言。
- 补齐快照读取、worker 状态读取、批次/任务读取、任务状态更新、任务尝试记录和日志列表查询接口；枚举、UUID、缓存字节数和缓存状态读取均做校验，未知值返回持久化错误。
- 清理改为按 cutoff 删除整个历史批次及其任务/尝试（包含 pending/running），保留设置、快照和磁盘缓存；增加级联清理测试。
- 修复无父目录数据库路径处理，并增加 `Database::open("updaddy.sqlite")` 回归测试。

### Fix round 1 验证

- `cargo fmt --manifest-path src-tauri/Cargo.toml`：通过。
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`：通过。
- `git diff --check`：通过。
- `cargo test --manifest-path src-tauri/Cargo.toml persistence:: -- --nocapture`：仍被 registry 阻塞，`objc2-app-kit ^0.6` 在当前索引中不存在（可用版本为 0.3/0.2）；未进入编译和测试阶段。

### Fix round 1 concerns

- 由于依赖版本解析阻塞，新增 Rust 测试无法在当前环境运行；依赖索引恢复后应重跑完整持久化测试。
- 磁盘缓存清理继续保守保留所有缓存行，避免缺少显式引用关系时误删最新缓存。
