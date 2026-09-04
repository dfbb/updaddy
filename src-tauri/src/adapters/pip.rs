use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{command, package_record, validate_name, EcosystemAdapter, ExecutorContext};
use super::parsers::json_array_versions;

#[derive(Debug, Clone, Default)]
pub struct PipAdapter;
impl PipAdapter {
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
impl EcosystemAdapter for PipAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Pip
    }
    async fn detect(&self, context: &ExecutorContext) -> Result<bool, TaskErrorKind> {
        match context
            .run(
                command("python", ["-m", "pip", "--version"]),
                CancellationToken::new(),
            )
            .await
        {
            Ok(result) if result.status.success() => Ok(true),
            Ok(result) => Err(self.classify_error(&result)),
            Err(_) => Ok(false),
        }
    }
    async fn scan(&self, context: &ExecutorContext) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        let installed = context
            .run(
                command("python", ["-m", "pip", "list", "--format=json"]),
                CancellationToken::new(),
            )
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !installed.status.success() {
            return Err(self.classify_error(&installed));
        }
        let outdated = context
            .run(
                command(
                    "python",
                    ["-m", "pip", "list", "--outdated", "--format=json"],
                ),
                CancellationToken::new(),
            )
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !outdated.status.success() {
            return Err(self.classify_error(&outdated));
        }
        let updates: std::collections::HashMap<_, _> = json_array_versions(&outdated.stdout)
            .map_err(|_| TaskErrorKind::CommandFailed)?
            .into_iter()
            .map(|(n, _, target)| (n, target))
            .collect();
        Ok(json_array_versions(&installed.stdout)
            .map_err(|_| TaskErrorKind::CommandFailed)?
            .into_iter()
            .map(|(name, current, _)| {
                package_record(
                    Ecosystem::Pip,
                    ResourceKind::Package,
                    name.clone(),
                    Some(current),
                    updates.get(&name).cloned().flatten(),
                )
            })
            .collect())
    }
    fn plan(&self, task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        if task.ecosystem != Ecosystem::Pip
            || !matches!(task.operation, Operation::Update | Operation::Uninstall)
        {
            return Err(TaskErrorKind::InvalidInput);
        }
        validate_name(&task.name)?;
        let args: Vec<String> = if matches!(task.operation, Operation::Update) {
            vec![
                "-m".into(),
                "pip".into(),
                "install".into(),
                "--upgrade".into(),
                task.name.clone(),
            ]
        } else {
            vec![
                "-m".into(),
                "pip".into(),
                "uninstall".into(),
                "--yes".into(),
                task.name.clone(),
            ]
        };
        Ok(command("python", args))
    }
    fn install_paths(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("python-site-packages")]
    }
    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}
