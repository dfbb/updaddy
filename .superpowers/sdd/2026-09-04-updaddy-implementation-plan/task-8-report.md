# Task 8 实施报告：SOCKS5 配置和 HTTP-to-SOCKS5 桥接

## 改动

- 新增 `src-tauri/src/proxy/{mod.rs,config.rs,env.rs,http_bridge.rs}`。
- `ProxyConfig` 解析并校验 `socks5`/`socks5h` URL、主机、端口和可选凭据；`Debug` 对密码脱敏。
- `ProxyRuntime` 执行固定 `example.com:443` SOCKS5 健康检查，按能力返回 `ALL_PROXY` 或 loopback `HTTP_PROXY`/`HTTPS_PROXY`，不回退直连；不支持能力返回 `ProxyUnsupported`。
- `HttpBridge` 仅绑定 `127.0.0.1:0`，支持 absolute-URI HTTP 和 CONNECT，并通过 SOCKS5 域名目标实现远程 DNS；记录活动连接并支持空闲关闭。
- 将 `ProxyEnv` 迁移到 proxy 模块，保留 adapters 兼容导出；executor allowlist 注入不变。
- `CommandSpec` 自定义 `Debug`，不显示环境值/完整代理 URL；worker 增加代理错误到现有可重试任务错误的映射。

## TDD 记录

RED：

```text
cargo test --manifest-path src-tauri/Cargo.toml proxy:: -- --nocapture
error[E0432]: unresolved imports ... ProxyConfig/ProxyRuntime/ProxyEnv/HttpBridge
```

GREEN（focused）：

```text
cargo test --manifest-path src-tauri/Cargo.toml proxy:: -- --nocapture
4 passed; 0 failed
cargo test --manifest-path src-tauri/Cargo.toml command_spec_debug_redacts_proxy_environment -- --nocapture
1 passed; 0 failed
```

## 完整验证

```text
cargo test --manifest-path src-tauri/Cargo.toml
49 unit tests + 8 cache_flow + 4 fake_commands + doc-tests passed; 0 failed
```

## 自审与安全

- 代理密码不进入 argv；`ProxyConfig`、`ProxyEnv`、`CommandSpec` 的 Debug 均不输出值；进程输出继续经过既有 `Redactor`（含 SOCKS URL 密码规则）。
- proxy 环境仅允许既有 allowlist；外部命令仍唯一通过 `ExecutorContext`/`ProcessSupervisor`。
- 健康检查、认证、桥接建立和网络错误归为 `ProxyUnavailable`/`BridgeFailed`（可重试映射 `ProxyDisconnected`）；配置错误和不支持能力不可重试。
- 未发现硬编码密钥、SQL 拼接、认证绕过、CORS/XSS 或依赖新增问题。工作树中既有未跟踪 `3rd/` 未纳入提交，未生成/提交 `dist/`。

## Concerns

- `ProxyRuntime::prepare(HttpOnly)` 每次首次准备会执行健康检查，批次级调用方应复用 runtime；`close_when_idle` 需在批次结束时显式调用。
- 当前 bridge 只重写请求行并保留 Host 等头部，未实现完整 HTTP 代理高级特性（按需求保持最小实现）。
