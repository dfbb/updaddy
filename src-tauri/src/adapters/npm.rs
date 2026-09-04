use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    command, home_path, package_record, validate_name, EcosystemAdapter, ExecutorContext,
};
use super::parsers::json_object_versions;

#[derive(Debug, Clone, Default)]
pub struct NpmAdapter;

impl NpmAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EcosystemAdapter for NpmAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    async fn run_with_context(
        &self,
        context: &ExecutorContext,
        command: crate::workers::WorkerCommand,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        self.execute_worker_command(context, command, cancel).await
    }

    async fn detect(&self, context: &ExecutorContext) -> Result<bool, TaskErrorKind> {
        match context
            .run(command("npm", ["--version"]), CancellationToken::new())
            .await
        {
            Ok(result) if result.status.success() => Ok(true),
            Ok(result) => Err(self.classify_error(&result)),
            Err(_) => Ok(false),
        }
    }

    async fn scan(&self, context: &ExecutorContext) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        self.scan_with_cancel(context, CancellationToken::new())
            .await
    }

    async fn scan_with_cancel(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        let installed = context
            .run(
                command("npm", ["ls", "--global", "--depth=0", "--json"]),
                cancel.clone(),
            )
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !installed.status.success() {
            return Err(self.classify_error(&installed));
        }
        let outdated = context
            .run(command("npm", ["outdated", "--global", "--json"]), cancel)
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        // npm exits 1 when outdated packages exist, so parse stdout regardless of status.
        if !outdated.status.success() && outdated.status.code() != Some(1) {
            return Err(self.classify_error(&outdated));
        }
        let updates = if outdated.stdout.trim().is_empty() {
            std::collections::HashMap::new()
        } else {
            json_object_versions(&outdated.stdout)
                .map_err(|_| self.classify_error(&outdated))?
                .into_iter()
                .map(|(n, _, target)| (n, target))
                .collect()
        };
        let mut records = Vec::new();
        let root = serde_json::from_str::<serde_json::Value>(&installed.stdout)
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if let Some(deps) = root.get("dependencies").and_then(|value| value.as_object()) {
            for (name, entry) in deps {
                let current = entry
                    .get("version")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                let target = updates.get(name).cloned().flatten();
                records.push(package_record(
                    Ecosystem::Npm,
                    ResourceKind::Package,
                    name,
                    current,
                    target,
                ));
            }
        }
        Ok(records)
    }

    fn plan(&self, task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        if task.ecosystem != Ecosystem::Npm
            || !matches!(task.operation, Operation::Update | Operation::Uninstall)
        {
            return Err(TaskErrorKind::InvalidInput);
        }
        validate_name(&task.name)?;
        let args: Vec<String> = if matches!(task.operation, Operation::Update) {
            vec!["install".into(), "--global".into(), task.name.clone()]
        } else {
            vec!["uninstall".into(), "--global".into(), task.name.clone()]
        };
        Ok(command("npm", args))
    }

    fn install_paths(&self) -> Vec<PathBuf> {
        let prefix = std::env::var_os("NPM_CONFIG_PREFIX").map(PathBuf::from);
        let mut paths = Vec::new();
        if let Some(prefix) = prefix {
            paths.push(prefix.join("lib/node_modules"));
        }
        paths.extend([
            home_path("~/.npm-global/lib/node_modules"),
            PathBuf::from("/usr/local/lib/node_modules"),
            PathBuf::from("/opt/homebrew/lib/node_modules"),
        ]);
        paths
    }
    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}

impl NpmAdapter {
    pub async fn discover_install_paths(
        &self,
        context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        let result = context
            .run(
                command("npm", ["root", "--global"]),
                CancellationToken::new(),
            )
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        let path = result
            .stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .ok_or(TaskErrorKind::CommandFailed)?;
        Ok(vec![PathBuf::from(path)])
    }
}
