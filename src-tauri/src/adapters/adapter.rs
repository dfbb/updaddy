use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{sink, CommandResult, CommandSpec, ProcessError, ProcessSupervisor};
use crate::workers::WorkerCommand;

/// Environment variables injected by the proxy runtime. Values are never put in argv.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProxyEnv {
    pub vars: HashMap<String, String>,
}

impl fmt::Debug for ProxyEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .vars
            .keys()
            .map(|key| (key.as_str(), "<redacted>"))
            .collect::<HashMap<_, _>>();
        formatter
            .debug_struct("ProxyEnv")
            .field("vars", &redacted)
            .finish()
    }
}

impl ProxyEnv {
    pub fn new(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            vars: vars.into_iter().collect(),
        }
    }
}

#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError>;
}

pub struct ProcessCommandRunner;

#[async_trait]
impl CommandRunner for ProcessCommandRunner {
    async fn run(
        &self,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError> {
        ProcessSupervisor::run(spec, cancel, sink()).await
    }
}

/// Dependencies shared by all adapters. A runner can be replaced by tests or a future proxy runtime.
#[derive(Clone)]
pub struct ExecutorContext {
    pub proxy: ProxyEnv,
    runner: Arc<dyn CommandRunner>,
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
            runner: Arc::new(ProcessCommandRunner),
        }
    }

    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            proxy: ProxyEnv::default(),
            runner,
        }
    }

    pub fn with_proxy(mut self, proxy: ProxyEnv) -> Self {
        self.proxy = proxy;
        self
    }

    pub async fn run(
        &self,
        mut spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError> {
        // ProcessSupervisor clears inherited variables. Restore only the allowlisted values
        // package managers need to resolve the active user's global installation.
        let path = std::env::var("PATH").unwrap_or_else(|_| {
            "/opt/homebrew/bin:/usr/local/bin:$HOME/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin"
                .replace("$HOME", &home_dir().to_string_lossy())
        });
        spec.env.entry("PATH".to_owned()).or_insert(path);
        spec.env
            .entry("HOME".to_owned())
            .or_insert_with(|| home_dir().to_string_lossy().into_owned());
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
        for (key, value) in &self.proxy.vars {
            spec.env.insert(key.clone(), value.clone());
        }
        self.runner.run(spec, cancel).await
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

    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        classify_command_error(result)
    }

    /// Compatibility bridge for the Task 5 worker queue.
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
            WorkerCommand::RefreshDiskUsage(_) => Err(TaskErrorKind::Unknown),
            WorkerCommand::Shutdown => Ok(()),
        }
    }
}

pub async fn execute_worker_command(
    adapter: &dyn EcosystemAdapter,
    context: &ExecutorContext,
    command: WorkerCommand,
    cancel: CancellationToken,
) -> Result<(), TaskErrorKind> {
    adapter
        .execute_worker_command(context, command, cancel)
        .await
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
    if name.contains('/') {
        if name.starts_with('/')
            || name.ends_with('/')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(TaskErrorKind::InvalidInput);
        }
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
            validate_segment(scope.trim_start_matches('@'), false)?;
            validate_segment(package, false)
        }
        (Ecosystem::Npm, ResourceKind::Package)
        | (Ecosystem::Pip, ResourceKind::Package)
        | (Ecosystem::Gem, ResourceKind::Package)
        | (Ecosystem::Rustup, ResourceKind::Toolchain)
        | (Ecosystem::Rustup, ResourceKind::Component)
        | (Ecosystem::Rustup, ResourceKind::Target)
        | (Ecosystem::Homebrew, ResourceKind::Formula)
        | (Ecosystem::Homebrew, ResourceKind::Cask) => validate_segment(name, false),
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
        || text.contains("readtimeout")
        || text.contains("read timeout")
        || text.contains("could not resolve host")
        || text.contains("could not resolve")
        || text.contains("dns lookup failed")
        || (text.contains("getaddrinfo")
            && (text.contains("enotfound") || text.contains("eai_again")))
        || text.contains("temporary failure in name resolution")
        || text.contains("name or service not known")
    {
        return TaskErrorKind::NetworkTimeout;
    }
    if text.contains("proxy")
        && (text.contains("disconnect")
            || text.contains("connection refused")
            || text.contains("cannot connect")
            || text.contains("connection reset")
            || text.contains("connection aborted")
            || text.contains("econnrefused")
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
        ProcessError::Cancelled | ProcessError::Io(_) | ProcessError::Spawn(_) => {
            TaskErrorKind::CommandFailed
        }
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
    }
}

pub fn operation_is_update(task: &PackageTask) -> bool {
    matches!(task.operation, Operation::Update)
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
}
