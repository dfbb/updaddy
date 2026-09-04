use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{command, package_record, validate_name, EcosystemAdapter, ExecutorContext};
use super::parsers::lines;

#[derive(Debug, Clone, Default)]
pub struct RustupAdapter;
impl RustupAdapter {
    pub fn new() -> Self {
        Self
    }

    pub async fn discover_install_paths(
        &self,
        _context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        Ok(<Self as EcosystemAdapter>::install_paths(self))
    }
}

#[async_trait]
impl EcosystemAdapter for RustupAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Rustup
    }
    async fn detect(&self, context: &ExecutorContext) -> Result<bool, TaskErrorKind> {
        match context
            .run(command("rustup", ["--version"]), CancellationToken::new())
            .await
        {
            Ok(result) if result.status.success() => Ok(true),
            Ok(result) => Err(self.classify_error(&result)),
            Err(_) => Ok(false),
        }
    }
    async fn scan(&self, context: &ExecutorContext) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        let toolchains = self.list(context, &["toolchain", "list"]).await?;
        let components = self
            .list(context, &["component", "list", "--installed"])
            .await?;
        let targets = self
            .list(context, &["target", "list", "--installed"])
            .await?;
        let mut records = Vec::new();
        for line in lines(&toolchains) {
            let name = line.split_whitespace().next().unwrap_or(line);
            records.push(package_record(
                Ecosystem::Rustup,
                ResourceKind::Toolchain,
                format!("toolchain:{name}"),
                None,
                None,
            ));
        }
        for line in lines(&components) {
            let name = line.split_whitespace().next().unwrap_or(line);
            records.push(package_record(
                Ecosystem::Rustup,
                ResourceKind::Component,
                format!("component:{name}"),
                None,
                None,
            ));
        }
        for line in lines(&targets) {
            let name = line.split_whitespace().next().unwrap_or(line);
            records.push(package_record(
                Ecosystem::Rustup,
                ResourceKind::Target,
                format!("target:{name}"),
                None,
                None,
            ));
        }
        Ok(records)
    }
    fn plan(&self, task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        if task.ecosystem != Ecosystem::Rustup {
            return Err(TaskErrorKind::InvalidInput);
        }
        let (kind, name) = if let Some(name) = task.name.strip_prefix("toolchain:") {
            (ResourceKind::Toolchain, name)
        } else if let Some(name) = task.name.strip_prefix("component:") {
            (ResourceKind::Component, name)
        } else if let Some(name) = task.name.strip_prefix("target:") {
            (ResourceKind::Target, name)
        } else {
            (ResourceKind::Toolchain, task.name.as_str())
        };
        if matches!(task.operation, Operation::Update) {
            return Ok(command("rustup", ["update"]));
        }
        if !matches!(task.operation, Operation::Uninstall) {
            return Err(TaskErrorKind::InvalidInput);
        }
        validate_name(name)?;
        let args = match kind {
            ResourceKind::Toolchain => vec!["toolchain", "uninstall", name],
            ResourceKind::Component => vec!["component", "remove", name],
            ResourceKind::Target => vec!["target", "remove", name],
            _ => return Err(TaskErrorKind::InvalidInput),
        };
        Ok(command("rustup", args))
    }
    fn install_paths(&self) -> Vec<PathBuf> {
        vec![
            PathBuf::from("~/.rustup/toolchains"),
            PathBuf::from("~/.rustup/toolchains/*/lib/rustlib"),
        ]
    }
    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}

impl RustupAdapter {
    async fn list(
        &self,
        context: &ExecutorContext,
        args: &[&str],
    ) -> Result<String, TaskErrorKind> {
        let result = context
            .run(command("rustup", args), CancellationToken::new())
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if result.status.success() {
            Ok(result.stdout)
        } else {
            Err(self.classify_error(&result))
        }
    }
}
