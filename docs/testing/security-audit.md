# updaddy 安全审计

审计日期：2026-09-05

审计范围：`src/`、`src-tauri/src/`、Tauri 配置、Rust/Node 锁文件及生产依赖。`3rd/` 是只读参考源码，不属于 updaddy 产物，也不纳入本次审计结论。

## 结论

未发现严重、高危或中危漏洞。发现两项已知低风险/信息性风险：代理密码按已确认设计存入本地 SQLite 而非 macOS Keychain；Cargo 跨平台锁文件包含不进入 Apple Silicon macOS 目标的 Linux GTK3 和旧宏依赖警告。

## 检查结果

| 检查项 | 结果 | 依据与处置 |
| --- | --- | --- |
| 硬编码密钥、令牌和凭据 | 未发现 | 对仓库源码执行常见 API key、私钥和密码模式扫描，无命中。测试中的 `secret` 仅为固定测试数据。 |
| SQL 注入 | 未发现 | `rusqlite` 查询和更新使用 `params!` 参数绑定；没有把前端输入拼接进 SQL。 |
| 身份认证和授权 | 不适用远程认证模型 | 应用是当前用户本地桌面程序，没有远程账户或服务端会话。Tauri capability 仅授予 `main` 窗口必要的 window/event/app/path 权限。 |
| 不安全的直接对象引用 | 未发现 | 任务 ID 使用 UUID 并通过本地数据库/worker 状态解析；不存在跨用户远程对象边界。 |
| 输入校验和命令注入 | 未发现 | 包名先按 Homebrew、npm、pip、Gem、rustup 各自语法校验，拒绝选项前缀、空白、路径穿越和 URL；子进程通过 `Command` 的独立 argv 启动，不使用 shell 拼接。 |
| 日志和错误中的敏感信息 | 未发现直接泄漏 | executor 在事件发送和持久化前脱敏已登记秘密、SOCKS5 URL 密码和 Bearer token；代理配置的 `Debug` 输出隐藏密码；状态栏不显示命令输出或代理信息。 |
| 依赖漏洞 | 未发现已知漏洞 | `cargo audit` 报告 `vulnerabilities.found=false`、0 个漏洞；此前完成的 `npm audit --omit=dev` 报告 0 个生产依赖漏洞。 |
| CORS 配置 | 不适用 | 应用不提供 HTTP API。内置代理 bridge 仅监听 `127.0.0.1` 随机端口，并要求随机 Basic 代理凭据；未认证请求返回 407。 |
| XSS | 未发现 | 前端没有 `dangerouslySetInnerHTML`、`innerHTML`、`eval` 或动态函数执行；日志作为文本在 `<pre>` 中渲染。CSP 将默认、脚本和连接来源限制在应用自身与 Tauri IPC。 |
| 其他严重数据泄露风险 | 未发现 | HTTP bridge 不向目标转发 `Proxy-*` 头；数据库目录和文件权限分别设置为 `0700` 和 `0600`；代理失败不会静默回退直连。 |

## 已知剩余风险

### 低风险：代理密码未进入 macOS Keychain

- 位置：`src-tauri/src/commands.rs` 的设置序列化，以及 `src-tauri/src/persistence/db.rs` 的 SQLite 存储。
- 风险：具有当前用户文件读取权限的本地进程、用户备份或已攻陷的用户会话可以读取代理密码。`0600` 文件权限能隔离其他普通系统用户，但不等同于系统密钥库保护。
- 当前控制：数据库父目录权限为 `0700`、文件权限为 `0600`；密码输入框不回显；日志和 `Debug` 输出会脱敏。设置页明确提示本地存储风险。
- 后续修复：若安全要求提升，将密码迁移到 macOS Keychain，SQLite 仅保存 Keychain 条目标识；迁移完成后清除旧字段。

### 信息性风险：跨平台传递依赖警告

- `cargo audit` 对完整 `Cargo.lock` 报告 17 条 allowed warning，主要是 Linux GTK3 bindings 未维护、`glib 0.18.5` 的 unsound 公告，以及旧 `proc-macro-error`/UNIC 依赖未维护。
- `cargo tree --target aarch64-apple-darwin -i glib` 返回 `nothing to print`，确认 GTK/glib 不进入本项目唯一支持的 Apple Silicon macOS 目标。
- 处置：随 Tauri/Wry 上游升级继续更新锁文件，并在发布前重复执行目标平台构建和 RustSec 审计。

## 验证命令

```bash
cargo audit --json
cargo tree --target aarch64-apple-darwin -i glib
npm audit --omit=dev
rg -n --hidden '<常见密钥和私钥模式>' .
rg -n 'dangerouslySetInnerHTML|innerHTML\s*=|eval\(|new Function' src src-tauri
```

本轮重新运行 npm 联网审计时，执行环境拒绝向公共 npm 注册表发送依赖元数据；报告中的 npm 结果来自同一实现完成后的此前成功执行。本次静态源码、锁文件和 RustSec 复核均已重新完成。
