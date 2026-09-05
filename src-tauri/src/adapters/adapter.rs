use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::RwLock;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::disk_usage::{resolve_package_paths, PackageInstallPaths};
use crate::executor::{
    sink, CommandResult, CommandSpec, EventSink, OutputChunkSink, ProcessError, ProcessSupervisor,
};
pub use crate::proxy::ProxyEnv;
use crate::proxy::{is_proxy_env_key, ProxyCapability, ProxyConfig, ProxyRuntime};
use crate::workers::WorkerCommand;

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError>;
}

pub struct ProcessCommandRunner {
    output_sink: EventSink,
}

impl Default for ProcessCommandRunner {
    fn default() -> Self {
        Self {
            output_sink: sink(),
        }
    }
}

#[async_trait]
impl CommandRunner for ProcessCommandRunner {
    async fn run(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError> {
        ProcessSupervisor::run(spec, cancel, self.output_sink.clone()).await
    }
}

/// Dependencies shared by all adapters. A runner can be replaced by tests or a future proxy runtime.
#[derive(Clone)]
pub struct ExecutorContext {
    pub proxy: ProxyEnv,
    runner: Arc<dyn CommandRunner>,
    proxy_runtime: Arc<RwLock<Option<Arc<ProxyRuntime>>>>,
    output_chunk_sink: Option<OutputChunkSink>,
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn home_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path == Path::new("~") {
        home_dir()
    } else if let Ok(stripped) = path.strip_prefix("~/") {
        home_dir().join(stripped)
    } else {
        path.to_path_buf()
    }
}

impl Default for ExecutorContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ExecutorContext {
    pub fn new() -> Self {
        Self {
            proxy: ProxyEnv::default(),
            runner: Arc::new(ProcessCommandRunner::default()),
            proxy_runtime: Arc::new(RwLock::new(None)),
            output_chunk_sink: None,
        }
    }

    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            proxy: ProxyEnv::default(),
            runner,
            proxy_runtime: Arc::new(RwLock::new(None)),
            output_chunk_sink: None,
        }
    }

    pub fn with_output_sink(output_sink: EventSink) -> Self {
        Self {
            proxy: ProxyEnv::default(),
            runner: Arc::new(ProcessCommandRunner { output_sink }),
            proxy_runtime: Arc::new(RwLock::new(None)),
            output_chunk_sink: None,
        }
    }

    pub fn with_proxy(mut self, proxy: ProxyEnv) -> Self {
        self.proxy = proxy;
        self
    }

    pub fn with_output_chunk_sink(mut self, sink: OutputChunkSink) -> Self {
        self.output_chunk_sink = Some(sink);
        self
    }

    pub fn set_proxy_config(&self, config: Option<ProxyConfig>) {
        let runtime = config.map(|config| Arc::new(ProxyRuntime::new(config)));
        *self
            .proxy_runtime
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = runtime;
    }

    pub async fn run(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError> {
        let mut spec = self.prepare_spec(spec);
        let capability = proxy_capability_for_program(&spec.program);
        let configured_runtime = self
            .proxy_runtime
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(runtime) = configured_runtime {
            let proxy = runtime
                .prepare(capability)
                .await
                .map_err(ProcessError::Proxy)?;
            for (key, value) in proxy.vars {
                spec.env.insert(key, value);
            }
        } else {
            for (key, value) in &self.proxy.vars {
                if is_proxy_env_key(key) {
                    spec.env.insert(key.clone(), value.clone());
                }
            }
        }
        self.runner.run(spec, cancel).await
    }

    /// 本地工具探测不依赖网络代理，避免代理离线时把已安装工具误判为不可用。
    pub async fn run_direct(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError> {
        self.runner.run(self.prepare_spec(spec), cancel).await
    }

    fn prepare_spec(&self, mut spec: CommandSpec) -> CommandSpec {
        // ProcessSupervisor clears inherited variables. Restore only the allowlisted values
        // package managers need to resolve the active user's global installation.
        // Trusted system and package-manager locations take precedence. The inherited PATH is
        // appended only for user-managed runtimes such as nvm/pyenv, so a writable directory
        // cannot shadow Homebrew or system executables.
        let mut path_entries = Vec::new();
        for required in [
            "/opt/homebrew/bin".to_owned(),
            "/usr/local/bin".to_owned(),
            home_dir().join(".cargo/bin").to_string_lossy().into_owned(),
            "/usr/bin".to_owned(),
            "/bin".to_owned(),
            "/usr/sbin".to_owned(),
            "/sbin".to_owned(),
        ] {
            if !path_entries.iter().any(|entry| entry == &required) {
                path_entries.push(required);
            }
        }
        for inherited in std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .filter(|entry| !entry.is_empty())
        {
            if !path_entries.iter().any(|entry| entry == inherited) {
                path_entries.push(inherited.to_owned());
            }
        }
        let path = path_entries.join(":");
        spec.env.entry("PATH".to_owned()).or_insert(path);
        spec.env
            .entry("HOME".to_owned())
            .or_insert_with(|| home_dir().to_string_lossy().into_owned());
        if spec.output_chunk_sink.is_none() {
            spec.output_chunk_sink = self.output_chunk_sink.clone();
        }
        for key in [
            "NPM_CONFIG_PREFIX",
            "GEM_HOME",
            "GEM_PATH",
            "VIRTUAL_ENV",
            "RUSTUP_HOME",
            "CARGO_HOME",
        ] {
            if let Some(value) = std::env::var_os(key) {
                spec.env
                    .entry(key.to_owned())
                    .or_insert_with(|| value.to_string_lossy().into_owned());
            }
        }
        spec
    }
}

fn proxy_capability_for_program(program: &str) -> ProxyCapability {
    match std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
    {
        // These tools consume HTTP(S)_PROXY but do not consistently consume SOCKS5 URLs.
        "brew" | "npm" | "python" | "python3" | "gem" => ProxyCapability::HttpOnly,
        "rustup" => ProxyCapability::NativeSocks5,
        _ => ProxyCapability::Unsupported,
    }
}

#[async_trait]
pub trait EcosystemAdapter: Send + Sync + 'static {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Homebrew
    }

    async fn detect(&self, _context: &ExecutorContext) -> Result<bool, TaskErrorKind> {
        Err(TaskErrorKind::Unknown)
    }

    async fn scan(&self, _context: &ExecutorContext) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        Err(TaskErrorKind::Unknown)
    }

    async fn scan_with_cancel(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        if cancel.is_cancelled() {
            return Err(TaskErrorKind::CommandFailed);
        }
        let result = self.scan(context).await;
        if cancel.is_cancelled() {
            Err(TaskErrorKind::CommandFailed)
        } else {
            result
        }
    }

    /// Build one argv-only command. Names are validated before they reach a command runner.
    fn plan(&self, _task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        Err(TaskErrorKind::Unknown)
    }

    async fn execute(
        &self,
        context: &ExecutorContext,
        task: &PackageTask,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        let spec = self.plan(task)?;
        let result = context
            .run(spec, cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if result.status.success() {
            Ok(())
        } else {
            Err(self.classify_error(&result))
        }
    }

    async fn uninstall(
        &self,
        context: &ExecutorContext,
        task: &PackageTask,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        self.execute(context, task, cancel).await
    }

    async fn update(
        &self,
        context: &ExecutorContext,
        task: &PackageTask,
        cancel: CancellationToken,
    ) -> Result<Vec<String>, TaskErrorKind> {
        self.run_with_context(context, WorkerCommand::Update(task.clone()), cancel)
            .await?;
        Ok(vec![task.name.clone()])
    }

    fn install_paths(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Resolve paths for the active global environment. Implementations may run their
    /// manager's introspection command; static paths remain a conservative fallback.
    async fn resolve_install_paths(
        &self,
        _context: &ExecutorContext,
        _cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        Ok(self.install_paths())
    }

    fn package_install_paths(
        &self,
        package: &PackageRecord,
        install_roots: &[PathBuf],
    ) -> PackageInstallPaths {
        resolve_package_paths(package, install_roots)
    }

    async fn resolve_package_install_paths(
        &self,
        _context: &ExecutorContext,
        package: &PackageRecord,
        install_roots: &[PathBuf],
        _cancel: CancellationToken,
    ) -> Result<PackageInstallPaths, TaskErrorKind> {
        Ok(self.package_install_paths(package, install_roots))
    }

    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        classify_command_error(result)
    }

    /// 兼容旧测试适配器；生产 worker 始终调用带上下文的路径。
    async fn run(
        &self,
        command: WorkerCommand,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        let context = ExecutorContext::new();
        self.execute_worker_command(&context, command, cancel).await
    }

    async fn run_with_context(
        &self,
        context: &ExecutorContext,
        command: WorkerCommand,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        self.execute_worker_command(context, command, cancel).await
    }

    async fn execute_worker_command(
        &self,
        context: &ExecutorContext,
        command: WorkerCommand,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        match command {
            WorkerCommand::Scan(ecosystem) => {
                if ecosystem != self.ecosystem() {
                    return Err(TaskErrorKind::InvalidInput);
                }
                self.scan_with_cancel(context, cancel).await.map(|_| ())
            }
            WorkerCommand::Update(task) => self.execute(context, &task, cancel).await,
            WorkerCommand::Uninstall(task) => self.uninstall(context, &task, cancel).await,
            WorkerCommand::Retry(_) => Err(TaskErrorKind::InvalidInput),
            WorkerCommand::RefreshDiskUsage(_) => Err(TaskErrorKind::Unknown),
            WorkerCommand::Shutdown => Ok(()),
        }
    }
}

pub fn validate_name(name: &str) -> Result<(), TaskErrorKind> {
    if name.is_empty()
        || name.len() > 256
        || name.starts_with('-')
        || name.contains('\\')
        || name.contains("://")
        || name.chars().any(|c| c.is_ascii_whitespace() || c == '\0')
        || name == "."
        || name == ".."
    {
        return Err(TaskErrorKind::InvalidInput);
    }
    if name.contains('/')
        && (name.starts_with('/')
            || name.ends_with('/')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(TaskErrorKind::InvalidInput);
    }
    Ok(())
}

fn validate_segment(name: &str, allow_at: bool) -> Result<(), TaskErrorKind> {
    validate_name(name)?;
    if name.starts_with('@') && !allow_at {
        return Err(TaskErrorKind::InvalidInput);
    }
    if !name.chars().all(|character| {
        character.is_ascii_alphanumeric()
            || "._-+".contains(character)
            || (allow_at && character == '@')
    }) {
        return Err(TaskErrorKind::InvalidInput);
    }
    Ok(())
}

/// Validate a resource according to the package manager's native name grammar.
/// Generic validation remains available for callers that only need path/option checks.
pub fn validate_resource_name(
    ecosystem: Ecosystem,
    kind: ResourceKind,
    name: &str,
) -> Result<(), TaskErrorKind> {
    match (ecosystem, kind) {
        (Ecosystem::Homebrew, ResourceKind::Tap) => {
            let mut parts = name.split('/');
            let owner = parts.next().unwrap_or_default();
            let repository = parts.next().unwrap_or_default();
            if parts.next().is_some() {
                return Err(TaskErrorKind::InvalidInput);
            }
            validate_segment(owner, false)?;
            validate_segment(repository, false)
        }
        (Ecosystem::Npm, ResourceKind::Package) if name.starts_with('@') => {
            let (scope, package) = name.split_once('/').ok_or(TaskErrorKind::InvalidInput)?;
            if scope.len() < 2 || scope[1..].contains('@') {
                return Err(TaskErrorKind::InvalidInput);
            }
            validate_segment(&scope[1..], false)?;
            validate_segment(package, false)
        }
        (Ecosystem::Npm, ResourceKind::Package)
        | (Ecosystem::Pip, ResourceKind::Package)
        | (Ecosystem::Gem, ResourceKind::Package)
        | (Ecosystem::Rustup, ResourceKind::Toolchain)
        | (Ecosystem::Rustup, ResourceKind::Component)
        | (Ecosystem::Rustup, ResourceKind::Target) => validate_segment(name, false),
        (Ecosystem::Homebrew, ResourceKind::Formula)
        | (Ecosystem::Homebrew, ResourceKind::Cask) => {
            if name.matches('@').count() > 1 {
                return Err(TaskErrorKind::InvalidInput);
            }
            validate_segment(name, true)
        }
        _ => validate_name(name),
    }
}

pub fn package_record(
    ecosystem: Ecosystem,
    kind: ResourceKind,
    name: impl Into<String>,
    current: Option<String>,
    target: Option<String>,
) -> PackageRecord {
    let name = name.into();
    PackageRecord {
        id: format!("{ecosystem:?}:{kind:?}:{name}"),
        ecosystem,
        resource_kind: kind,
        name,
        current_version: current,
        target_version: target.clone(),
        disk_usage: None,
        update_available: target.is_some(),
    }
}

pub fn classify_command_error(result: &CommandResult) -> TaskErrorKind {
    let text = format!("{}\n{}", result.stderr, result.stdout).to_ascii_lowercase();
    if result.status.code() == Some(28) {
        return TaskErrorKind::NetworkTimeout;
    }
    if (text.contains("timed out")
        && (text.contains("connect")
            || text.contains("network")
            || text.contains("operation")
            || text.contains("read")))
        || text.contains("connect timeout")
        || text.contains("etimedout")
        || text.contains("open_timeout")
        || text.contains("open timeout")
        || text.contains("readtimeout")
        || text.contains("read timeout")
        || text.contains("could not resolve host")
        || text.contains("could not resolve registry")
        || text.contains("dns lookup failed")
        || text.contains("eai_again")
        || (text.contains("getaddrinfo")
            && (text.contains("enotfound") || text.contains("eai_again")))
        || text.contains("temporary failure in name resolution")
        || text.contains("name or service not known")
    {
        return TaskErrorKind::NetworkTimeout;
    }
    if (text.contains("proxy") || text.contains("socks"))
        && (text.contains("disconnect")
            || text.contains("connection refused")
            || text.contains("connection failed")
            || text.contains("cannot connect")
            || text.contains("connection reset")
            || text.contains("connection aborted")
            || text.contains("econnrefused")
            || text.contains("econnreset")
            || text.contains("econnaborted")
            || text.contains("handshake failed")
            || text.contains("broken pipe")
            || text.contains("closed"))
    {
        return TaskErrorKind::ProxyDisconnected;
    }
    let http_codes = [
        "500", "501", "502", "503", "504", "505", "506", "507", "508", "509", "510", "511",
    ];
    let http_5xx = text.lines().any(|line| {
        line.contains("http")
            && http_codes.iter().any(|code| {
                line.split_whitespace()
                    .any(|token| token.trim_matches(|c: char| !c.is_ascii_digit()) == *code)
            })
    }) || http_codes.iter().any(|code| {
        text.contains(&format!("{code} service unavailable"))
            || text.contains(&format!("{code} bad gateway"))
            || text.contains(&format!("returned error: {code}"))
    });
    if http_5xx {
        return TaskErrorKind::HttpServerTemporaryError;
    }
    if text.contains("permission denied") || text.contains("eacces") {
        return TaskErrorKind::PermissionDenied;
    }
    if text.contains("invalid") || text.contains("unknown option") || text.contains("usage:") {
        return TaskErrorKind::InvalidInput;
    }
    TaskErrorKind::CommandFailed
}

pub fn classify_process_error(error: &ProcessError) -> TaskErrorKind {
    match error {
        ProcessError::Spawn(source) if source.kind() == std::io::ErrorKind::NotFound => {
            TaskErrorKind::CommandFailed
        }
        ProcessError::Spawn(source) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            TaskErrorKind::PermissionDenied
        }
        // A supervisor timeout has no evidence that the cause was the network; do not
        // retry arbitrary long-running or hung local commands.
        ProcessError::ProcessTimeout => TaskErrorKind::CommandFailed,
        ProcessError::Proxy(proxy) => match proxy {
            crate::proxy::ProxyError::InvalidConfig
            | crate::proxy::ProxyError::ProxyUnsupported => TaskErrorKind::InvalidInput,
            crate::proxy::ProxyError::ProxyUnavailable | crate::proxy::ProxyError::BridgeFailed => {
                TaskErrorKind::ProxyDisconnected
            }
        },
        ProcessError::Cancelled | ProcessError::Io(_) | ProcessError::Spawn(_) => {
            TaskErrorKind::CommandFailed
        }
    }
}

pub fn detect_process_error(error: &ProcessError) -> Result<bool, TaskErrorKind> {
    match error {
        ProcessError::Spawn(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        other => Err(classify_process_error(other)),
    }
}

pub fn command(program: &str, args: impl IntoIterator<Item = impl AsRef<str>>) -> CommandSpec {
    CommandSpec {
        program: program.to_owned(),
        args: args
            .into_iter()
            .map(|arg| arg.as_ref().to_owned())
            .collect(),
        env: HashMap::new(),
        cwd: None,
        stdin: None,
        pseudo_terminal: false,
        output_chunk_sink: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn result(stderr: &str) -> CommandResult {
        CommandResult {
            status: Command::new("sh").arg("-c").arg("exit 1").status().unwrap(),
            stdout: String::new(),
            stderr: stderr.to_owned(),
            duration: std::time::Duration::ZERO,
        }
    }

    #[test]
    fn only_transient_network_failures_are_retryable() {
        assert_eq!(
            classify_command_error(&result("Could not resolve host")),
            TaskErrorKind::NetworkTimeout
        );
        assert_eq!(
            classify_command_error(&result("proxy connection refused")),
            TaskErrorKind::ProxyDisconnected
        );
        assert_eq!(
            classify_command_error(&result("HTTP 503")),
            TaskErrorKind::HttpServerTemporaryError
        );
        assert_eq!(
            classify_command_error(&result("HTTP error 503 Service Unavailable")),
            TaskErrorKind::HttpServerTemporaryError
        );
        assert_eq!(
            classify_command_error(&result("permission denied")),
            TaskErrorKind::PermissionDenied
        );
        assert!(!classify_command_error(&result("invalid option")).is_retryable());
        assert!(!classify_command_error(&result("request timeout setting")).is_retryable());
        assert!(
            !classify_command_error(&result("ERESOLVE could not resolve dependency tree"))
                .is_retryable()
        );
    }

    #[test]
    fn names_cannot_escape_argv_validation() {
        assert!(validate_name("lodash").is_ok());
        assert_eq!(
            validate_name("--prefix=/tmp"),
            Err(TaskErrorKind::InvalidInput)
        );
        assert_eq!(validate_name("bad name"), Err(TaskErrorKind::InvalidInput));
        assert_eq!(validate_name(""), Err(TaskErrorKind::InvalidInput));
    }

    #[test]
    fn resource_validation_keeps_scoped_and_tap_names_but_rejects_specs() {
        assert!(
            validate_resource_name(Ecosystem::Npm, ResourceKind::Package, "@scope/pkg").is_ok()
        );
        assert!(
            validate_resource_name(Ecosystem::Homebrew, ResourceKind::Tap, "owner/repo").is_ok()
        );
        assert!(
            validate_resource_name(Ecosystem::Homebrew, ResourceKind::Formula, "python@3.12")
                .is_ok()
        );
        assert!(
            validate_resource_name(Ecosystem::Pip, ResourceKind::Package, "file:../pkg").is_err()
        );
        assert!(validate_resource_name(Ecosystem::Npm, ResourceKind::Package, "../pkg").is_err());
    }

    #[test]
    fn proxy_debug_redacts_values_and_rejects_non_proxy_keys() {
        let proxy = ProxyEnv::new([
            (
                "HTTPS_PROXY".into(),
                "socks5://user:secret@127.0.0.1:1080".into(),
            ),
            ("HOME".into(), "/private".into()),
        ]);
        let debug = format!("{proxy:?}");
        assert!(!debug.contains("secret"));
        assert!(!proxy.vars.contains_key("HOME"));
    }
}
