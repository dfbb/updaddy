# updaddy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 构建一个面向最新 macOS Apple Silicon 的 Tauri 2 桌面应用 `updaddy`，统一扫描、更新和卸载 Homebrew、npm、pip、Gem、rustup 五个生态，并提供 SOCKS5 代理、磁盘占用缓存、计划任务、状态栏驻留、主题和 i18n。

**Architecture:** 前端采用 React + TypeScript + Vite，Rust 后端采用 Tauri 2 单应用。Rust 维护五个独立生态 worker，每个 worker 在独立线程中串行处理本生态的扫描、更新、卸载和磁盘测量；不同生态 worker 可按资源锁并行。前端通过 Tauri command 提交任务，通过 Tauri 事件通道接收状态、进度、日志和磁盘占用，不使用前端轮询。

**Tech Stack:** Rust 2021、Tauri 2、Tokio、Serde、Rusqlite、React 19、TypeScript、Vite、`js-yaml`、Lucide React、macOS 原生状态栏和登录项 API。

**Spec:** `docs/superpowers/specs/2026-09-03-updaddy-design.md`

本规格包含多个技术模块，但它们共享同一套 `PackageTask`、worker 和 Tauri 事件契约，拆成相互独立的计划会重复定义接口并增加集成风险。因此保留一个产品计划，按可独立测试的垂直任务拆分；每个任务都有自己的测试和提交点。

## Global Constraints

- 只支持最新 macOS 和 Apple Silicon。
- 首版只支持 Homebrew、npm、pip、Gem、rustup 五个生态；不支持 Yarn、uv、Cargo 全局二进制和项目级依赖。
- Homebrew 管理 formula、cask、tap；npm、pip、Gem 只管理全局安装资源；rustup 管理工具链、组件和 target。
- 用户隐藏生态后暂停完整扫描和计划更新，并从概览和状态栏总数排除；应用启动时只对隐藏生态执行轻量 `detect()`。
- 不同生态可以并行；Homebrew 使用 `brew` 锁，npm 使用 `node-global` 锁，pip 使用 `python-global` 锁，Gem 使用 `ruby-global` 锁，rustup 使用 `rustup` 锁。
- 仅网络超时、代理断连、HTTP 服务端临时错误自动重试，最多 3 次，间隔 5 秒、30 秒、120 秒；权限、参数、工具缺失和版本冲突不自动重试。
- 代理只支持一个 SOCKS5 地址，可选用户名和密码；代理不可用时不得静默直连；HTTP 桥接器只监听 `127.0.0.1`。
- 代理凭据存于应用 SQLite 配置文件，不使用 macOS Keychain；配置目录和文件限制为当前用户读写，日志必须脱敏。
- 每个生态 tab 只有一个软件包列表；默认待更新优先，按磁盘占用排序时对整个列表排序；每个软件包有独立卸载按钮，待更新包另有独立更新按钮。
- 扫描、更新或卸载任务期间，所有扫描、更新、卸载、刷新和一键更新按钮置灰；活动生态 tab 顶部进度条和取消按钮保持可用。
- 磁盘占用首次发现时计算，缓存缺失、版本/路径变化、更新成功或用户单包刷新时只计算受影响软件包；不重复全量扫描。
- 语言资源必须位于 `lang/en.yaml` 和 `lang/zh_CN.yaml`，首版支持英文和简体中文。
- 日志和历史保留 90 天，过期记录自动清理。
- 不修改 `3rd/Depvault`、`3rd/topgrade`；`dist/` 必须被 `.gitignore` 忽略，不进入提交。

## File Map

### 工程和前端

- Create: `package.json`, `tsconfig.json`, `tsconfig.node.json`, `vite.config.ts`, `vitest.config.ts`, `index.html`
- Create: `src/main.tsx`, `src/App.tsx`, `src/types.ts`, `src/api/tauri.ts`, `src/state/appStore.ts`
- Create: `src/test/setup.ts`
- Create: `src/i18n/catalog.ts`, `src/i18n/useI18n.ts`, `src/theme/theme.ts`, `src/styles/tokens.css`, `src/styles/app.css`
- Create: `src/components/AppShell.tsx`, `src/components/OverviewPage.tsx`, `src/components/EcosystemTabs.tsx`, `src/components/PackageTable.tsx`, `src/components/TaskProgressBar.tsx`, `src/components/ConfirmUninstallDialog.tsx`, `src/components/SettingsPage.tsx`, `src/components/HistoryPage.tsx`
- Create: `src/components/LogViewer.tsx`, `src/components/EmptyState.tsx`, `src/components/StatusBadge.tsx`
- Create: `lang/en.yaml`, `lang/zh_CN.yaml`

### Rust 后端

- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`
- Create: `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`
- Create: `src-tauri/src/core/{mod.rs,models.rs,errors.rs,events.rs}`
- Create: `src-tauri/src/persistence/{mod.rs,db.rs,migrations.rs,repositories.rs,cleanup.rs}`
- Create: `src-tauri/src/executor/{mod.rs,process.rs,redaction.rs}`
- Create: `src-tauri/src/workers/{mod.rs,worker.rs,messages.rs,supervisor.rs}`
- Create: `src-tauri/src/adapters/{mod.rs,adapter.rs,homebrew.rs,npm.rs,pip.rs,gem.rs,rustup.rs,parsers.rs}`
- Create: `src-tauri/src/disk_usage/{mod.rs,measure.rs,cache.rs}`
- Create: `src-tauri/src/proxy/{mod.rs,config.rs,http_bridge.rs,env.rs}`
- Create: `src-tauri/src/scheduler/{mod.rs,schedule.rs,catch_up.rs}`
- Create: `src-tauri/src/platform/{mod.rs,macos.rs,tray.rs,login_item.rs}`
- Create: `src-tauri/src/commands.rs`, `src-tauri/src/event_bus.rs`, `src-tauri/migrations/0001_initial.sql`
- Create: `src-tauri/tests/{fake_commands.rs,worker_flow.rs,cache_flow.rs}`

## Task 1: 初始化可运行的 Tauri 工程

**Files:**
- Create: `package.json`, `tsconfig.json`, `tsconfig.node.json`, `vite.config.ts`, `vitest.config.ts`, `index.html`
- Create: `src/main.tsx`, `src/App.tsx`, `src/styles/tokens.css`, `src/styles/app.css`
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`

**Interfaces:**
- Produces: 可执行的 `npm run build`、`cargo check` 和 `npm run tauri dev` 基础工程。
- Produces: `src-tauri/src/lib.rs` 导出 `run() -> tauri::Result<()>`，供 `main.rs` 调用。

- [ ] **Step 1: 写入前端依赖和脚本**

在 `package.json` 中固定 React、Tauri API、`js-yaml`、`lucide-react` 和 Vite/TypeScript 构建脚本：

```json
{
  "scripts": { "dev": "vite", "build": "tsc -b && vite build", "tauri": "tauri" },
  "dependencies": {
    "@tauri-apps/api": "^2.10.1",
    "js-yaml": "^4.1.0",
    "lucide-react": "^0.468.0",
    "react": "^19.2.4",
    "react-dom": "^19.2.4"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.10.1",
    "@testing-library/jest-dom": "^6.6.3",
    "@testing-library/react": "^16.1.0",
    "@testing-library/user-event": "^14.5.2",
    "@types/js-yaml": "^4.0.9",
    "@types/react": "^19.1.2",
    "@types/react-dom": "^19.1.2",
    "@vitejs/plugin-react": "^6.0.1",
    "jsdom": "^26.0.0",
    "typescript": "~6.0.2",
    "vite": "^8.0.3",
    "vitest": "^3.0.5"
  }
}
```

在 `src-tauri/Cargo.toml` 中加入与后续接口对应的依赖：

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-autostart = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
uuid = { version = "1", features = ["serde", "v4"] }
chrono = { version = "0.4", features = ["serde", "clock"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "process", "io-util", "net", "fs"] }
tokio-util = { version = "0.7", features = ["rt"] }
async-trait = "0.1"
rusqlite = { version = "0.32", features = ["bundled"] }
walkdir = "2"
tokio-socks = "0.5"
url = "2"
libc = "0.2"

[dev-dependencies]
tempfile = "3"

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = "0.6"
objc2-app-kit = "0.6"
```

- [ ] **Step 2: 创建最小 React 壳和稳定样式变量**

`src/main.tsx` 渲染 `App`，`src/App.tsx` 先显示应用标题和加载状态；`src/styles/tokens.css` 定义浅色、深色和系统主题所需的语义变量，不使用固定 viewport 字体缩放。`vitest.config.ts` 配置 React JSX、`jsdom` 环境和 `src/test/setup.ts` 的 `@testing-library/jest-dom` 初始化。

- [ ] **Step 3: 配置 Tauri 构建和最小能力集**

在 `tauri.conf.json` 中配置 `productName: "updaddy"`、`identifier: "com.dfbb.updaddy"`、`frontendDist: "../dist"`、Apple Silicon bundle 目标；`capabilities/default.json` 仅声明窗口、事件和应用目录权限。

- [ ] **Step 4: 验证工程构建**

运行：

```bash
npm install
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

预期：三个命令均成功，`dist/` 生成但被 `.gitignore` 忽略。

- [ ] **Step 5: Commit**

```bash
git add package.json tsconfig.json tsconfig.node.json vite.config.ts index.html src src-tauri
git commit -m "chore: bootstrap tauri app"
```

## Task 2: 建立领域模型、错误分类和事件协议

**Files:**
- Create: `src-tauri/src/core/models.rs`, `src-tauri/src/core/errors.rs`, `src-tauri/src/core/events.rs`, `src-tauri/src/core/mod.rs`
- Test: `src-tauri/src/core/models.rs` 内的 `#[cfg(test)]` 模块

**Interfaces:**
- Produces: `Ecosystem`, `ResourceKind`, `Operation`, `PackageRecord`, `PackageTask`, `OperationBatch`, `TaskStatus`, `DiskUsage`, `LogEntry`, `TaskErrorKind`。
- Produces: 可序列化事件 `WorkerStateEvent`, `TaskProgressEvent`, `PackageChangedEvent`, `DiskUsageEvent`, `LogEntryEvent`, `BatchSummaryEvent`。

- [ ] **Step 1: 写领域模型失败测试**

```rust
#[test]
fn package_task_serializes_update_and_uninstall_operations() {
    let task = PackageTask::new(Ecosystem::Npm, "typescript", Operation::Update);
    assert_eq!(task.operation, Operation::Update);
    assert_eq!(serde_json::to_value(&task).unwrap()["ecosystem"], "npm");
}

#[test]
fn task_error_kind_marks_only_transient_errors_retryable() {
    assert!(TaskErrorKind::NetworkTimeout.is_retryable());
    assert!(TaskErrorKind::ProxyDisconnected.is_retryable());
    assert!(!TaskErrorKind::PermissionDenied.is_retryable());
}
```

- [ ] **Step 2: 实现枚举和结构体**

使用 `#[serde(rename_all = "snake_case")]`，实现以下核心字段：

```rust
pub enum Ecosystem { Homebrew, Npm, Pip, Gem, Rustup }
pub enum ResourceKind { Formula, Cask, Tap, Package, Toolchain, Component, Target }
pub enum Operation { Scan, Update, Uninstall, MeasureDisk }
pub enum DiskUsageStatus { Ready, Measuring, Unavailable }
pub struct PackageRecord {
    pub id: String,
    pub ecosystem: Ecosystem,
    pub resource_kind: ResourceKind,
    pub name: String,
    pub current_version: Option<String>,
    pub target_version: Option<String>,
    pub disk_usage: Option<DiskUsage>,
    pub update_available: bool,
}
pub struct DiskUsage { pub bytes: u64, pub scanned_at: i64, pub status: DiskUsageStatus }
pub struct LogEntry { pub message: String, pub emitted_at: i64, pub stream: String }
impl PackageTask {
    pub fn new(ecosystem: Ecosystem, name: impl Into<String>, operation: Operation) -> Self;
}
impl TaskErrorKind {
    pub fn is_retryable(&self) -> bool;
}
```

- [ ] **Step 3: 定义事件 payload 和错误分类**

为每个事件加入 `ecosystem`、`sequence: u64` 和 `emitted_at`；任务级事件额外加入 `task_id: Uuid`，批次事件额外加入 `batch_id: Uuid`，让前端能够丢弃旧事件并在恢复时按序重建。`TaskErrorKind::is_retryable()` 只对网络超时、代理断连和 HTTP 5xx 返回 `true`。

- [ ] **Step 4: 运行领域测试**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core:: -- --nocapture
```

预期：序列化和重试分类测试通过。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core
git commit -m "feat: add updaddy domain and event models"
```

## Task 3: 实现 SQLite 持久化和磁盘缓存表

**Files:**
- Create: `src-tauri/migrations/0001_initial.sql`
- Create: `src-tauri/src/persistence/{mod.rs,db.rs,migrations.rs,repositories.rs,cleanup.rs}`
- Test: `src-tauri/src/persistence/{repositories.rs,cleanup.rs}` 内的测试模块

**Interfaces:**
- Consumes: `core::{PackageRecord, PackageTask, OperationBatch, LogEntry, DiskUsage}`。
- Produces: `Database::open(path) -> Result<Database>`、`save_snapshot`、`upsert_disk_usage`、`create_batch`、`append_log`、`mark_running_tasks_interrupted`、`cleanup_before(cutoff) -> Result<u64>`。
- Produces: `DiskUsageCacheEntry::ready(ecosystem, package_id, version, root, bytes)` and `LogEntry::at(message, unix_seconds)` test constructors.

- [ ] **Step 1: 写迁移和仓储失败测试**

```rust
#[test]
fn disk_usage_cache_is_reused_for_same_package_version_and_root() {
    let db = Database::open(tempfile::tempdir().unwrap().path().join("updaddy.sqlite")).unwrap();
    let entry = DiskUsageCacheEntry::ready("npm", "eslint", "9.0.0", "/global", 4096);
    db.upsert_disk_usage(&entry).unwrap();
    assert_eq!(db.find_disk_usage("npm", "eslint", "9.0.0", "/global").unwrap().unwrap().bytes, 4096);
}

#[test]
fn cleanup_removes_records_older_than_cutoff() {
    let db = Database::open(tempfile::tempdir().unwrap().path().join("updaddy.sqlite")).unwrap();
    db.append_log(&LogEntry::at("old", 1_600_000_000)).unwrap();
    assert_eq!(db.cleanup_before(1_700_000_000).unwrap(), 1);
}
```

- [ ] **Step 2: 创建 0001 初始 schema**

创建 `settings`、`worker_state`、`manager_profiles`、`package_snapshots`、`disk_usage_cache`、`operation_batches`、`package_tasks`、`task_attempts`、`log_entries`、`scheduler_state` 表。`disk_usage_cache` 主键为 `(ecosystem, package_id, installed_version, install_root)`，保存 `path_signature`、`bytes`、`status` 和 `scanned_at`。

- [ ] **Step 3: 实现数据库初始化和权限**

`Database::open` 创建 `~/Library/Application Support/updaddy`，设置目录 `0700`、数据库文件 `0600`，运行迁移并启用 SQLite WAL。所有写入通过 `Mutex<Connection>` 在事务内完成。

- [ ] **Step 4: 实现仓储和恢复逻辑**

实现快照、worker 状态、批次、任务、尝试、日志和缓存的读写；应用启动时将状态为 `running` 的任务原子更新为 `interrupted`，不自动重放。

- [ ] **Step 5: 实现 90 天清理**

`cleanup_before` 删除过期的 `operation_batches`、`package_tasks`、`task_attempts` 和 `log_entries`，保留 `settings`、`package_snapshots` 和仍被引用的最新缓存。

- [ ] **Step 6: 运行持久化测试并提交**

```bash
cargo test --manifest-path src-tauri/Cargo.toml persistence:: -- --nocapture
git add src-tauri/migrations src-tauri/src/persistence
git commit -m "feat: add sqlite persistence and disk cache"
```

## Task 4: 实现统一子进程执行器和日志脱敏

**Files:**
- Create: `src-tauri/src/executor/{mod.rs,process.rs,redaction.rs}`
- Test: `src-tauri/src/executor/{process.rs,redaction.rs}` 内的测试模块

**Interfaces:**
- Produces: `CommandSpec { program, args, env, cwd, stdin }`、`CommandResult { status, stdout, stderr, duration }`。
- Produces: `ProcessSupervisor::run(spec, cancel_token, event_sink) -> Result<CommandResult>`。
- Produces: `Redactor::redact(text, secrets) -> String`、测试构造器 `CommandSpec::for_test(program, args)` 和 `sink()`。

- [ ] **Step 1: 写执行器失败测试**

```rust
#[tokio::test]
async fn command_result_captures_stdout_and_exit_code() {
    let result = ProcessSupervisor::run(CommandSpec::for_test("printf", &["ok"]), CancellationToken::new(), sink()).await.unwrap();
    assert_eq!(result.stdout, "ok");
    assert_eq!(result.status.code(), Some(0));
}

#[test]
fn redactor_removes_proxy_password_and_bearer_token() {
    let text = "socks5://user:secret@example.test:1080 Bearer abc123";
    assert!(!Redactor::redact(text, &["secret", "abc123"]).contains("secret"));
}
```

- [ ] **Step 2: 实现进程组和超时**

在 macOS 使用 `CommandExt::process_group(0)` 创建独立进程组；通过 Tokio 读取 stdout/stderr。命令执行超过超时上限返回 `ProcessTimeout`，只有底层连接明确报告网络超时才映射为 `NetworkTimeout`，避免把本地卡死误判为可重试网络错误。所有子进程必须设置 `TERM=dumb`、明确工作目录和受控环境。

- [ ] **Step 3: 实现取消**

收到 `CancellationToken` 时先向进程组发送 SIGTERM，等待 3 秒，再发送 SIGKILL；返回 `TaskStatus::Cancelled`，不把用户取消当作可重试错误。

- [ ] **Step 4: 实现结构化输出和脱敏**

每行输出生成时间戳和 stream 字段；写入数据库和事件前脱敏 SOCKS5 URL、认证 token、密码和配置中登记的秘密值。前端只接收文本字段。

- [ ] **Step 5: 运行执行器测试并提交**

```bash
cargo test --manifest-path src-tauri/Cargo.toml executor:: -- --nocapture
git add src-tauri/src/executor
git commit -m "feat: add cancellable process executor"
```

## Task 5: 实现五个独立生态 worker 和任务监督器

**Files:**
- Create: `src-tauri/src/workers/{mod.rs,worker.rs,messages.rs,supervisor.rs}`
- Test: `src-tauri/src/workers/{worker.rs,supervisor.rs}` 内的测试模块

**Interfaces:**
- Consumes: `CommandSpec`、`PackageTask`、`EcosystemAdapter`、`Database`、事件 sink。
- Produces: `WorkerSupervisor::start(context) -> WorkerSupervisor`、`submit(WorkerCommand) -> TaskId`、`cancel(TaskId) -> Result<()>`。
- Produces: `WorkerCommand::{Scan,Update,Uninstall,RefreshDiskUsage,Shutdown}`。

- [ ] **Step 1: 写 worker 并行和取消失败测试**

```rust
#[test]
fn different_ecosystem_workers_can_run_at_the_same_time() {
    let (supervisor, events) = SupervisorHarness::with_barrier();
    let brew = supervisor.submit(WorkerCommand::Scan(Ecosystem::Homebrew)).unwrap();
    let npm = supervisor.submit(WorkerCommand::Scan(Ecosystem::Npm)).unwrap();
    events.wait_for_running(brew);
    events.wait_for_running(npm);
}

#[test]
fn same_ecosystem_commands_are_serialized() {
    let (supervisor, events) = SupervisorHarness::with_delayed_adapter();
    let first = supervisor.submit(WorkerCommand::Update(package_task(Ecosystem::Pip, "a"))).unwrap();
    let second = supervisor.submit(WorkerCommand::Update(package_task(Ecosystem::Pip, "b"))).unwrap();
    events.wait_for_started(first);
    assert!(!events.has_started(second));
    events.release_first_task();
    events.wait_for_completed(first);
    events.wait_for_started(second);
}
```

- [ ] **Step 2: 实现每生态独立线程**

`WorkerSupervisor` 启动五个 `std::thread::spawn` worker。每个 worker 在线程内创建 current-thread Tokio runtime，消费自己的 `WorkerCommand` 队列；同一生态队列串行，不共享可变 adapter 状态。

测试模块中实现 `SupervisorHarness::with_barrier() -> (WorkerSupervisor, TestEvents)`、`SupervisorHarness::with_delayed_adapter() -> (WorkerSupervisor, TestEvents)`、`package_task(ecosystem, name) -> PackageTask`、`TestEvents::wait_for_running`、`wait_for_completed`、`wait_for_started`、`has_started` 和 `release_first_task`，使上述测试完全在内存假适配器上运行。

- [ ] **Step 3: 实现资源锁和全局活动状态**

worker 使用 `brew`、`node-global`、`python-global`、`ruby-global`、`rustup` 五个锁键；监督器维护活动 worker 数和活动任务集合，发送 `worker-state` 事件供前端统一置灰操作按钮。

- [ ] **Step 4: 接入任务取消和批次状态**

`submit` 立即返回 UUID；worker 将任务写入 `package_tasks`，执行期间发送进度，结束时写入结果。`cancel` 取消队列中的任务或触发执行器的进程组取消。

- [ ] **Step 5: 验证 worker 流程并提交**

```bash
cargo test --manifest-path src-tauri/Cargo.toml workers:: -- --nocapture
git add src-tauri/src/workers
git commit -m "feat: add per-ecosystem workers"
```

## Task 6: 实现 Homebrew、npm、pip、Gem、rustup 适配器

**Files:**
- Create: `src-tauri/src/adapters/{mod.rs,adapter.rs,homebrew.rs,npm.rs,pip.rs,gem.rs,rustup.rs,parsers.rs}`
- Test: `src-tauri/tests/fake_commands.rs` 和各适配器文件内测试

**Interfaces:**
- Produces: `#[async_trait] trait EcosystemAdapter`，方法为 `ecosystem`、`detect`、`scan`、`plan`、`execute`、`uninstall`、`install_paths`、`classify_error`。
- Consumes: `ExecutorContext`、`ProxyEnv`、`PackageTask`。

- [ ] **Step 1: 写统一适配器契约测试**

为每个生态准备假命令脚本，固定 stdout、stderr、退出码和交互输入；测试 `detect`、`scan`、`execute`、`uninstall` 都不依赖真实全局环境。

- [ ] **Step 2: 实现 Homebrew 适配器**

使用 `brew outdated --formula`、`brew outdated --cask` 和 tap 信息扫描；formula/cask 分别支持升级和卸载，tap 使用 `brew update` 检测变化、`brew untap` 卸载。`install_paths` 返回 Cellar、cask artifact 和 tap 仓库路径。

- [ ] **Step 3: 实现 npm 适配器**

使用 `npm ls --global --depth=0 --json` 和 `npm outdated --global --json` 获取包信息；更新使用包级全局命令，卸载使用 `npm uninstall --global <name>`；`install_paths` 使用 `npm root --global` 下的包目录。

- [ ] **Step 4: 实现 pip 适配器**

使用当前 Python 的 `python -m pip list --outdated --format=json` 和 `python -m pip list --format=json`；更新使用 `python -m pip install --upgrade <name>`，卸载使用 `python -m pip uninstall --yes <name>`；不自动 sudo，不访问项目目录。

- [ ] **Step 5: 实现 Gem 适配器**

使用当前 `gem` 的全局列表和 outdated 输出；更新使用 `gem update <name>`，卸载使用指定版本的 `gem uninstall <name> --version <version>`；不读取 Bundler 项目、不修改 Gemfile、不自动 sudo；`install_paths` 使用 `gem env gemdir` 和 gemspec 文件清单。

- [ ] **Step 6: 实现 rustup 适配器**

使用 `rustup toolchain list`、`rustup component list --installed` 和 `rustup target list --installed`；更新使用 `rustup update`，卸载分别使用 `rustup toolchain uninstall`、`rustup component remove`、`rustup target remove`。不实现 cargo 二进制更新。

- [ ] **Step 7: 实现错误分类和适配器测试**

按 stderr/退出码将 DNS、连接超时、代理断连和 HTTP 5xx 映射为可重试；参数、权限、工具不存在和版本冲突映射为确定性错误。

```bash
cargo test --manifest-path src-tauri/Cargo.toml adapters:: -- --nocapture
```

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src/adapters src-tauri/tests/fake_commands.rs
git commit -m "feat: add package manager adapters"
```

## Task 7: 实现磁盘占用测量和缓存失效策略

**Files:**
- Create: `src-tauri/src/disk_usage/{mod.rs,measure.rs,cache.rs}`
- Modify: `src-tauri/src/workers/worker.rs`, `src-tauri/src/adapters/adapter.rs`
- Test: `src-tauri/tests/cache_flow.rs`, `src-tauri/src/disk_usage/{measure.rs,cache.rs}` 内测试

**Interfaces:**
- Produces: `DiskUsageService::measure(package, paths) -> Result<DiskUsage>`、`measure_if_needed(package, cached) -> Result<DiskUsage>`、`should_measure(package, cached) -> bool`、`invalidate_after_success(package)`、`CacheStore::in_memory()`。
- Consumes: 适配器的 `install_paths`、`disk_usage_cache` 仓储和 `DiskUsageEvent`。

- [ ] **Step 1: 写缓存命中、失效和去重测试**

```rust
#[test]
fn cache_miss_measures_only_the_requested_package() {
    let fixture = DiskFixture::new().with_package("a", 4096).with_package("b", 8192);
    let service = DiskUsageService::new(fixture.walker());
    let result = service.measure_if_needed(package("a"), None).unwrap();
    assert_eq!(result.bytes, 4096);
    assert_eq!(fixture.walk_count("a"), 1);
    assert_eq!(fixture.walk_count("b"), 0);
}

#[test]
fn successful_update_invalidates_only_updated_package() {
    let cache = CacheStore::in_memory();
    cache.put(cache_entry("a", 100));
    cache.put(cache_entry("b", 200));
    cache.invalidate_after_success(&package("a")).unwrap();
    assert!(cache.get(&package("a")).unwrap().is_none());
    assert_eq!(cache.get(&package("b")).unwrap().unwrap().bytes, 200);
}

#[test]
fn hard_links_are_counted_once_and_symlinks_are_not_followed() {
    let fixture = UnixDiskFixture::with_file("real", b"1234")
        .with_hard_link("hard", "real")
        .with_symlink("link", "real");
    assert_eq!(measure_paths(&[fixture.root()]).unwrap().bytes, 4);
}
```

- [ ] **Step 2: 实现文件测量器**

使用 `walkdir` 遍历适配器返回的路径，只统计普通文件 `metadata.len()`，不跟随符号链接；使用 `(st_dev, st_ino)` 去重硬链接。多个路径的总和写入 `u64` 字节数。

在测试模块中提供 `DiskFixture::new`、`with_package`、`walker`、`walk_count`、`package`、`cache_entry`、`UnixDiskFixture::with_file`、`with_hard_link`、`with_symlink` 和 `measure_paths`，让缓存和文件系统行为可重复验证。

- [ ] **Step 3: 实现缓存键和失效判断**

缓存键使用生态、包 ID、已安装版本和安装根；计算实际路径签名。命中相同版本、根目录和路径签名时直接返回缓存；缺失、版本变化、根目录变化、路径签名变化、更新成功或单包刷新时只测量该包。

- [ ] **Step 4: 接入首次扫描和更新后的单包测量**

扫描适配器返回列表后，为每个包提交 `MeasureDisk` 子任务；使用有界并发。更新成功后只提交被更新包，卸载成功后删除其快照和缓存。

- [ ] **Step 5: 运行缓存测试并提交**

```bash
cargo test --manifest-path src-tauri/Cargo.toml disk_usage cache_flow -- --nocapture
git add src-tauri/src/disk_usage src-tauri/src/workers/worker.rs src-tauri/src/adapters/adapter.rs src-tauri/tests/cache_flow.rs
git commit -m "feat: add package disk usage cache"
```

## Task 8: 实现 SOCKS5 配置和 HTTP-to-SOCKS5 桥接

**Files:**
- Create: `src-tauri/src/proxy/{mod.rs,config.rs,http_bridge.rs,env.rs}`
- Modify: `src-tauri/src/executor/process.rs`, `src-tauri/src/workers/worker.rs`
- Test: `src-tauri/src/proxy/{config.rs,http_bridge.rs,env.rs}` 内测试

**Interfaces:**
- Produces: `ProxyConfig { mode, host, port, username, password }`、`ProxyConfig::parse(url) -> Result<ProxyConfig>`、`ProxyRuntime::new(config)`、`ProxyRuntime::prepare(capability) -> ProxyEnv`、`ProxyRuntime::health_check()`、`HttpBridge::start(config) -> BridgeHandle`、`ProxyCapability` 和 `ProxyError`。

- [ ] **Step 1: 写代理解析和安全失败测试**

```rust
#[test]
fn proxy_url_parses_optional_credentials() {
    let config = ProxyConfig::parse("socks5://alice:secret@example.test:1080").unwrap();
    assert_eq!(config.host, "example.test");
    assert_eq!(config.port, 1080);
    assert_eq!(config.username.as_deref(), Some("alice"));
}

#[tokio::test]
async fn enabled_proxy_never_returns_direct_fallback_when_bridge_fails() {
    let runtime = ProxyRuntime::new(ProxyConfig::test_unreachable());
    let result = runtime.prepare(ProxyCapability::HttpOnly).await;
    assert!(matches!(result, Err(ProxyError::ProxyUnavailable)));
}
```

- [ ] **Step 2: 实现配置验证和健康检查**

拒绝空主机、非法端口和不支持的 scheme；代理模式开启时执行 SOCKS5 握手、认证和轻量连通性测试。密码只保存在内存对象和 SQLite 配置字段中，不出现在 `Debug` 输出。

在测试模块中实现 `ProxyConfig::test_unreachable()`，固定使用 `127.0.0.1:1`，保证失败测试不访问外部网络。

- [ ] **Step 3: 实现直连、原生 SOCKS5 和 HTTP 环境注入**

支持 SOCKS5/`ALL_PROXY` 的工具注入 SOCKS5 地址；HTTP-only 工具接收桥接器的 `HTTP_PROXY`/`HTTPS_PROXY`；不支持代理的适配器返回 `ProxyUnsupported`。

- [ ] **Step 4: 实现 loopback HTTP 桥接器**

绑定 `127.0.0.1:0`，支持普通 HTTP 和 CONNECT，使用 SOCKS5 远程 DNS 转发。桥接句柄记录随机端口和活动连接数，批次结束且连接归零时关闭。

- [ ] **Step 5: 运行代理测试并提交**

```bash
cargo test --manifest-path src-tauri/Cargo.toml proxy:: -- --nocapture
git add src-tauri/src/proxy src-tauri/src/executor/process.rs src-tauri/src/workers/worker.rs
git commit -m "feat: add socks5 proxy bridge"
```

## Task 9: 实现每日/每周计划和睡眠补执行

**Files:**
- Create: `src-tauri/src/scheduler/{mod.rs,schedule.rs,catch_up.rs}`
- Modify: `src-tauri/src/workers/supervisor.rs`, `src-tauri/src/persistence/repositories.rs`
- Test: `src-tauri/src/scheduler/{schedule.rs,catch_up.rs}` 内测试

**Interfaces:**
- Produces: `Schedule::{Daily{time},Weekly{weekday,time}}`、`Scheduler::start()`、`Scheduler::next_due(now)`、`CatchUp::should_run(cycle_id, now, state, schedule) -> bool`、`SchedulerState::record_cycle(cycle_id)`、`EnabledEcosystems::only(list)`。

- [ ] **Step 1: 写可控时钟测试**

```rust
#[test]
fn missed_daily_schedule_runs_once_after_wake() {
    let schedule = Schedule::daily("09:00").unwrap();
    let state = SchedulerState::default();
    let first = CatchUp::should_run("2026-09-04", unix("2026-09-04T10:00:00+08:00"), &state, &schedule);
    assert!(first);
    let state = state.record_cycle("2026-09-04");
    assert!(!CatchUp::should_run("2026-09-04", unix("2026-09-04T10:05:00+08:00"), &state, &schedule));
}

#[test]
fn hidden_ecosystem_is_excluded_from_scheduled_batch() {
    let visible = EnabledEcosystems::only(&[Ecosystem::Npm]);
    let tasks = Scheduler::plan_visible_updates(visible, snapshot_with_all_ecosystems());
    assert!(tasks.iter().all(|task| task.ecosystem == Ecosystem::Npm));
}
```

- [ ] **Step 2: 实现每日/每周计算**

将本地时区的时分和星期转换为下一次 due 时间；周期 ID 使用计划配置版本和周期起始时间组合，写入 `scheduler_state`。

测试模块提供 `unix(iso8601) -> i64`、`SchedulerState::record_cycle`、`EnabledEcosystems::only` 和 `snapshot_with_all_ecosystems`，使计划测试不依赖系统当前时间或真实包管理器。

- [ ] **Step 3: 实现启动、唤醒和批次互斥**

应用启动和 macOS 唤醒事件都调用一次 `should_run`；隐藏生态只执行轻量 detect。若手动批次正在运行，计划批次等待其结束后重新扫描，不重复创建。

- [ ] **Step 4: 运行 scheduler 测试并提交**

```bash
cargo test --manifest-path src-tauri/Cargo.toml scheduler:: -- --nocapture
git add src-tauri/src/scheduler src-tauri/src/workers/supervisor.rs src-tauri/src/persistence/repositories.rs
git commit -m "feat: add scheduled updates and catch-up"
```

## Task 10: 实现 Tauri commands、事件总线和 macOS 状态栏/登录项

**Files:**
- Create: `src-tauri/src/commands.rs`, `src-tauri/src/event_bus.rs`, `src-tauri/src/platform/{mod.rs,macos.rs,tray.rs,login_item.rs}`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`
- Test: `src-tauri/tests/worker_flow.rs`

**Interfaces:**
- Produces commands: `scan_ecosystem`, `scan_all_visible`, `update_package`, `uninstall_package`, `update_all_visible`, `cancel_task`, `get_state_snapshot`, `get_settings`, `save_settings`, `set_login_item`, `open_settings`。
- Produces events: `worker-state`, `task-progress`, `package-changed`, `disk-usage`, `log-entry`, `batch-summary`。
- Produces: `AppState::test_with_blocked_worker()`、`PackageId` 和 `TaskId` command 参数模型。

- [ ] **Step 1: 写 command 非阻塞测试**

```rust
#[test]
fn update_command_returns_task_id_without_waiting_for_worker_completion() {
    let app = AppState::test_with_blocked_worker();
    let task_id = submit_update_command(&app, package_id("npm", "eslint")).unwrap();
    assert!(task_id != Uuid::nil());
    assert!(app.worker_events().is_empty());
}
```

- [ ] **Step 2: 实现 command 到 supervisor 的映射**

command 只校验可见生态、包 ID 和任务互斥状态，然后向对应 worker 投递消息并返回 UUID；不得在 Tauri command future 中执行包管理器命令。

测试模块中实现 `AppState::test_with_blocked_worker()`、`package_id(ecosystem, name) -> PackageId` 和 `submit_update_command(&AppState, PackageId) -> Result<TaskId>`；blocked worker 用 barrier 保证 command 返回前不会完成。

- [ ] **Step 3: 实现事件总线和快照恢复**

`EventBus` 将结构化事件写入 SQLite 后通过 `AppHandle::emit` 推送；`get_state_snapshot` 返回当前 worker、包、任务和日志状态，用于窗口重新打开或事件丢失后的单次恢复。

- [ ] **Step 4: 实现状态栏菜单**

使用 Tauri tray API 创建模板图标和菜单：动态显示五个启用生态的总待更新数、手动一键更新、打开主窗口、设置、退出。数量项点击打开概览。

- [ ] **Step 5: 实现关闭隐藏、退出和登录项**

关闭窗口改为隐藏；退出先取消未开始任务并终止活动子进程，写最终状态后退出。使用 macOS 登录项 API 注册/撤销开机启动，不自动打开主窗口。

- [ ] **Step 6: 接入睡眠/唤醒通知并提交**

通过 macOS `NSWorkspace`/系统通知监听唤醒事件，调用 scheduler 的 catch-up；验证状态栏和后台 worker 在窗口隐藏时仍存在。

```bash
cargo test --manifest-path src-tauri/Cargo.toml worker_flow -- --nocapture
git add src-tauri/src/commands.rs src-tauri/src/event_bus.rs src-tauri/src/platform src-tauri/src/lib.rs src-tauri/src/main.rs src-tauri/tauri.conf.json src-tauri/capabilities/default.json src-tauri/tests/worker_flow.rs
git commit -m "feat: add tauri commands and macos lifecycle"
```

## Task 11: 实现前端事件状态、i18n 和主题

**Files:**
- Create: `lang/en.yaml`, `lang/zh_CN.yaml`
- Create: `src/types.ts`, `src/api/tauri.ts`, `src/state/appStore.ts`, `src/i18n/catalog.ts`, `src/i18n/useI18n.ts`, `src/theme/theme.ts`
- Modify: `src/App.tsx`, `src/styles/tokens.css`, `src/styles/app.css`
- Test: `src/i18n/catalog.test.ts`, `src/state/appStore.test.ts`

**Interfaces:**
- Produces: `useAppStore()`、`invokeTask(command, payload) -> Promise<TaskId>`、`subscribeToBackendEvents()`、`t(key, vars)`、`setTheme(mode)`。
- Consumes: Rust commands和 Tauri events中定义的 snake_case JSON payload。

- [ ] **Step 1: 创建完整翻译键和加载测试**

在两个 YAML 文件中定义相同的键树，包括 `overview`、五个生态名、任务状态、按钮、设置、日志和卸载确认文本；测试缺失中文键回退到英文。

```ts
it("falls back to English for a missing locale key", () => {
  const translator = createTranslator({ zh_CN: { task: {} }, en: { task: { cancel: "Cancel" } } });
  expect(translator.t("task.cancel", "zh_CN")).toBe("Cancel");
});
```

- [ ] **Step 2: 实现 Tauri 事件订阅**

`src/api/tauri.ts` 使用 `invoke`、`listen`；监听 `worker-state`、`task-progress`、`package-changed`、`disk-usage`、`log-entry`、`batch-summary`，按 sequence 丢弃旧事件。窗口恢复时调用 `get_state_snapshot` 一次，不创建轮询定时器。

`catalog.test.ts` 中实现 `createTranslator(catalogs) -> { t(key, locale): string }` 测试工厂；生产实现使用同一键查找和英文回退规则。

- [ ] **Step 3: 实现全局状态 store**

store 保存生态可见性、检测状态、包列表、活动任务、总待更新数、主题、语言和日志配置；任何 worker active 时派生 `operationsDisabled=true`，但活动任务的取消按钮仍可用。

- [ ] **Step 4: 实现主题和系统语言**

主题支持 `system`、`light`、`dark`；系统 locale 映射 `zh-CN` 到 `zh_CN`，其他 locale 回退 `en`。主题变化同步 `document.documentElement.dataset.theme`。

- [ ] **Step 5: 运行前端测试并提交**

```bash
npm run build
npx vitest run src/i18n/catalog.test.ts src/state/appStore.test.ts
git add lang src/types.ts src/api src/state src/i18n src/theme src/App.tsx src/styles
git commit -m "feat: add frontend event state and localization"
```

## Task 12: 实现概览、五个生态 tab、列表操作和进度条

**Files:**
- Create: `src/components/AppShell.tsx`, `src/components/OverviewPage.tsx`, `src/components/EcosystemTabs.tsx`, `src/components/PackageTable.tsx`, `src/components/TaskProgressBar.tsx`, `src/components/ConfirmUninstallDialog.tsx`, `src/components/StatusBadge.tsx`, `src/components/EmptyState.tsx`
- Modify: `src/App.tsx`, `src/state/appStore.ts`, `src/styles/app.css`
- Test: `src/components/PackageTable.test.tsx`, `src/components/OverviewPage.test.tsx`

**Interfaces:**
- Consumes: `useAppStore`、`update_package`、`uninstall_package`、`scan_ecosystem`、`cancel_task`。
- Produces: 五个独立生态 tab；每个 tab 单一列表、扫描按钮、逐包更新/卸载按钮、顶部活动任务进度和取消。

- [ ] **Step 1: 写列表排序和按钮状态测试**

```tsx
const normalPackage = { id: "npm:prettier", name: "prettier", updateAvailable: false, diskBytes: 1024 };
const updateablePackage = { id: "npm:eslint", name: "eslint", updateAvailable: true, diskBytes: 2048 };
const smallUpdate = { id: "npm:a", name: "a", updateAvailable: true, diskBytes: 100 };
const largeNormal = { id: "npm:b", name: "b", updateAvailable: false, diskBytes: 10_000 };

it("puts updateable packages first by default", () => {
  render(<PackageTable packages={[normalPackage, updateablePackage]} />);
  expect(screen.getAllByRole("row")[1]).toHaveTextContent(updateablePackage.name);
});

it("sorts the entire list by disk bytes when the disk column is selected", async () => {
  render(<PackageTable packages={[smallUpdate, largeNormal]} />);
  await userEvent.click(screen.getByRole("button", { name: /disk usage/i }));
  expect(screen.getAllByRole("row")[1]).toHaveTextContent(largeNormal.name);
});
```

- [ ] **Step 2: 实现 tab 和隐藏规则**

显示 `概览`、Homebrew、npm、pip、Gem、rustup、`历史与日志`、`设置`；检测不到工具或用户关闭显示开关的生态不渲染 tab。概览和状态栏只统计可见生态。

- [ ] **Step 3: 实现单列表和排序**

列为名称、当前版本、目标版本、磁盘占用、状态、操作。默认比较 `(update_available desc, name asc)`；磁盘列比较 `diskBytes` 全列表，不保留更新优先；再次点击默认排序恢复更新优先。

- [ ] **Step 4: 实现逐包更新和卸载确认**

有更新的包显示更新按钮，每个包显示卸载按钮。卸载对话框显示生态、包名、版本和资源类型，确认后才调用 command；同包已有活动任务时两个按钮均禁用。

- [ ] **Step 5: 实现活动 tab 顶部进度和全局置灰**

活动 worker 的 tab 顶部固定显示 determinate/indeterminate 进度、当前操作、已完成数和取消按钮。任何 worker active 时禁用所有扫描、更新、卸载、刷新和一键更新按钮；导航、日志查看、关闭窗口和取消仍可用。

- [ ] **Step 6: 运行组件测试并提交**

```bash
npx vitest run src/components/PackageTable.test.tsx src/components/OverviewPage.test.tsx
npm run build
git add src/components src/App.tsx src/state/appStore.ts src/styles/app.css
git commit -m "feat: add ecosystem tabs and package actions"
```

## Task 13: 实现设置双 tab、历史和日志查看/配置

**Files:**
- Create: `src/components/SettingsPage.tsx`, `src/components/HistoryPage.tsx`, `src/components/LogViewer.tsx`
- Modify: `src/components/AppShell.tsx`, `src/types.ts`, `src/state/appStore.ts`, `src/i18n/catalog.ts`
- Test: `src/components/SettingsPage.test.tsx`, `src/components/HistoryPage.test.tsx`, `src/components/LogViewer.test.tsx`

**Interfaces:**
- Consumes: `get_settings`、`save_settings`、`get_state_snapshot`、历史/日志事件。
- Produces: `SettingsPage({ saveSettings })`、常规设置和日志设置的表单验证、历史筛选和脱敏日志展示。

- [ ] **Step 1: 写设置表单测试**

```tsx
const mockSaveSettings = vi.fn().mockResolvedValue(undefined);

it("saves daily schedule and hides an ecosystem", async () => {
  render(<SettingsPage saveSettings={mockSaveSettings} />);
  await userEvent.selectOptions(screen.getByLabelText(/schedule/i), "daily");
  await userEvent.click(screen.getByLabelText(/show gem/i));
  await userEvent.click(screen.getByRole("button", { name: /save/i }));
  expect(mockSaveSettings).toHaveBeenCalledWith(expect.objectContaining({ schedule: { kind: "daily" }, visible: expect.objectContaining({ gem: false }) }));
});
```

- [ ] **Step 2: 实现常规 tab**

提供 system/light/dark、直连/SOCKS5、每日/每周固定时间、开机启动和五个生态显示开关。隐藏生态立即暂停完整扫描和计划任务；保存设置后发送 supervisor 更新。

- [ ] **Step 3: 实现日志 tab**

显示结构化日志和命令输出，配置日志级别、是否保留完整命令输出，并提供立即清理过期日志按钮；保留周期固定 90 天。日志文本使用纯文本渲染。

- [ ] **Step 4: 实现历史筛选和重试入口**

按时间、生态、批次类型和状态筛选；失败任务重试先调用对应生态重新扫描，再提交新 `PackageTask`。卸载和更新批次都显示尝试记录。

- [ ] **Step 5: 运行设置和日志测试并提交**

```bash
npx vitest run src/components/SettingsPage.test.tsx src/components/HistoryPage.test.tsx src/components/LogViewer.test.tsx
npm run build
git add src/components/SettingsPage.tsx src/components/HistoryPage.tsx src/components/LogViewer.tsx src/components/AppShell.tsx src/types.ts src/state src/i18n
git commit -m "feat: add settings history and logs"
```

## Task 14: 集成验证、macOS smoke test 和发布检查

**Files:**
- Modify: `src-tauri/tests/{worker_flow.rs,cache_flow.rs}`、`src-tauri/tauri.conf.json`、`.gitignore`
- Create: `docs/testing/updaddy-macos-smoke.md`

**Interfaces:**
- Consumes: 所有前述 Rust、Tauri 和前端接口。
- Produces: 可重复的本地测试命令和 macOS Apple Silicon 验收清单。

- [ ] **Step 1: 运行完整 Rust 和前端测试**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
npx vitest run
```

- [ ] **Step 2: 验证 dist 和参考项目隔离**

```bash
git check-ignore -v dist/test-artifact
git status --short
```

预期：`dist/test-artifact` 命中根目录 `.gitignore`；`3rd/` 没有进入暂存区。

- [ ] **Step 3: 执行 macOS smoke test**

先创建 `docs/testing/updaddy-macos-smoke.md`，写入以下逐项清单，然后在最新 macOS Apple Silicon 上逐项执行：

```markdown
- [ ] 状态栏显示五个启用生态的合计更新数，数量项点击打开概览
- [ ] 状态栏和主窗口都能打开设置
- [ ] 关闭主窗口后状态栏和后台 worker 继续运行
- [ ] 登录项开关能启动/撤销后台应用
- [ ] 睡眠唤醒后错过计划只补执行一次
- [ ] 隐藏生态启动时只做 detect，不扫描包和磁盘
- [ ] 代理认证失败时任务失败且没有直连流量
- [ ] HTTP bridge 只监听 127.0.0.1
- [ ] 每个软件包的更新和卸载按钮行为正确，卸载前有确认
- [ ] 任务运行时所有操作按钮置灰，活动 tab 顶部进度和取消按钮可用
- [ ] 应用重启后中断任务标记为 interrupted，历史和缓存仍可读取
```

- [ ] **Step 4: 执行安全和依赖检查**

```bash
cargo audit
npm audit --omit=dev
rg -n "(sk-|ghp_|AKIA|password|token|secret)" src src-tauri lang --glob '!*.test.*'
```

检查输出不得包含硬编码凭据；确认日志脱敏、SQLite 权限和桥接器监听地址符合规格。

- [ ] **Step 5: 构建 Apple Silicon 产物并确认 dist 未跟踪**

```bash
npm run tauri -- build --target aarch64-apple-darwin
git status --short --ignored | sed -n '1,120p'
```

预期：构建成功，产物位于 `dist/` 或 Tauri bundle 输出目录，所有 dist 文件保持 ignored。

- [ ] **Step 6: Commit 验收文档**

```bash
git add docs/testing/updaddy-macos-smoke.md src-tauri/tests .gitignore
git commit -m "test: add updaddy integration checks"
```

## Plan Self-Review

### Spec coverage

- 五个生态、全局范围和不支持 Yarn/uv/Cargo：Tasks 1、6、12。
- 独立 worker、资源锁、并行、取消、前台不阻塞：Tasks 4、5、10、12。
- 更新、卸载、确认、历史和重试：Tasks 2、5、6、10、12、13。
- 磁盘占用首次扫描、缓存命中、单包失效和排序：Tasks 3、7、12。
- SOCKS5、认证、HTTP bridge、不可静默直连：Task 8。
- 每日/每周计划、睡眠补执行、隐藏生态启动探测：Task 9、10、13。
- 状态栏、设置入口、登录项、窗口隐藏：Task 10、13。
- 主题、i18n、`lang/*.yaml`：Task 11、12、13。
- 日志脱敏、90 天清理、恢复和安全检查：Tasks 3、4、13、14。
- `dist/` 忽略和 `3rd/` 隔离：Tasks 1、14。

### Placeholder scan

计划没有使用 `TODO`、`TBD`、未决占位或“稍后实现”步骤；每个任务均包含具体文件、接口、测试命令和提交点。

### Type and interface consistency

- `PackageTask`、`OperationBatch`、`WorkerCommand` 和事件名称在 Tasks 2、5、10、11、12 中保持一致。
- `disk_usage_cache` 的主键和 `DiskUsageService` 的失效条件在 Tasks 3、7、12 中保持一致。
- 前端 command 名称与 Rust command 列表一致；事件 payload 使用统一 sequence 字段。
