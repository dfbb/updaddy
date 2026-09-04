use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    classify_process_error, command, home_path, package_record, validate_resource_name,
    EcosystemAdapter, ExecutorContext,
};
use super::parsers::lines;

#[derive(Debug, Clone, Default)]
pub struct HomebrewAdapter;

impl HomebrewAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn run(
        &self,
        context: &ExecutorContext,
        spec: CommandSpec,
        cancel: CancellationToken,
    ) -> Result<CommandResult, TaskErrorKind> {
        let result = context
            .run(spec, cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if result.status.success() {
            Ok(result)
        } else {
            Err(self.classify_error(&result))
        }
    }

    fn task_kind(name: &str) -> (ResourceKind, &str) {
        if let Some(value) = name.strip_prefix("cask:") {
            (ResourceKind::Cask, value)
        } else if let Some(value) = name.strip_prefix("tap:") {
            (ResourceKind::Tap, value)
        } else {
            (ResourceKind::Formula, name)
        }
    }

    pub async fn discover_install_paths(
        &self,
        _context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        Ok(<Self as EcosystemAdapter>::install_paths(self))
    }
}

#[async_trait]
impl EcosystemAdapter for HomebrewAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Homebrew
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
            .run(command("brew", ["--version"]), CancellationToken::new())
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
        // Refresh metadata before reading either installed or outdated resources.
        let tap_refresh = self
            .run(context, command("brew", ["update"]), cancel.clone())
            .await?;
        let formula_installed = self
            .run(
                context,
                command("brew", ["list", "--formula", "--versions"]),
                cancel.clone(),
            )
            .await?;
        let cask_installed = self
            .run(
                context,
                command("brew", ["list", "--cask", "--versions"]),
                cancel.clone(),
            )
            .await?;
        let formula = self
            .run(
                context,
                command("brew", ["outdated", "--formula"]),
                cancel.clone(),
            )
            .await?;
        let cask = self
            .run(
                context,
                command("brew", ["outdated", "--cask"]),
                cancel.clone(),
            )
            .await?;
        let taps = self.run(context, command("brew", ["tap"]), cancel).await?;
        let outdated_formula = lines(&formula.stdout)
            .map(|line| line.split_whitespace().next().unwrap_or(line))
            .collect::<std::collections::HashSet<_>>();
        let outdated_cask = lines(&cask.stdout)
            .map(|line| line.split_whitespace().next().unwrap_or(line))
            .collect::<std::collections::HashSet<_>>();
        let mut records = Vec::new();
        for (name, current) in installed_versions(&formula_installed.stdout) {
            records.push(package_record(
                Ecosystem::Homebrew,
                ResourceKind::Formula,
                name,
                current,
                outdated_formula
                    .contains(name.as_str())
                    .then(|| "latest".into()),
            ));
        }
        for (name, current) in installed_versions(&cask_installed.stdout) {
            records.push(package_record(
                Ecosystem::Homebrew,
                ResourceKind::Cask,
                format!("cask:{name}"),
                current,
                outdated_cask
                    .contains(name.as_str())
                    .then(|| "latest".into()),
            ));
        }
        let changed_taps = changed_taps(&tap_refresh.stdout);
        for name in lines(&taps.stdout) {
            records.push(package_record(
                Ecosystem::Homebrew,
                ResourceKind::Tap,
                format!("tap:{name}"),
                None,
                changed_taps.contains(name).then(|| "updated".into()),
            ));
        }
        Ok(records)
    }

    fn plan(&self, task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        if task.ecosystem != Ecosystem::Homebrew {
            return Err(TaskErrorKind::InvalidInput);
        }
        let (kind, name) = Self::task_kind(&task.name);
        validate_resource_name(Ecosystem::Homebrew, kind, name)?;
        let args: Vec<String> = match (task.operation, kind) {
            (crate::core::Operation::Update, ResourceKind::Formula) => {
                vec!["upgrade".into(), name.into()]
            }
            (crate::core::Operation::Update, ResourceKind::Cask) => {
                vec!["upgrade".into(), "--cask".into(), name.into()]
            }
            (crate::core::Operation::Update, ResourceKind::Tap) => vec!["update".into()],
            (crate::core::Operation::Uninstall, ResourceKind::Formula) => {
                vec!["uninstall".into(), name.into()]
            }
            (crate::core::Operation::Uninstall, ResourceKind::Cask) => {
                vec!["uninstall".into(), "--cask".into(), name.into()]
            }
            (crate::core::Operation::Uninstall, ResourceKind::Tap) => {
                vec!["untap".into(), name.into()]
            }
            _ => return Err(TaskErrorKind::InvalidInput),
        };
        Ok(command("brew", args))
    }

    async fn uninstall(
        &self,
        context: &ExecutorContext,
        task: &PackageTask,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        let mut uninstall_task = task.clone();
        uninstall_task.operation = crate::core::Operation::Uninstall;
        self.execute(context, &uninstall_task, cancel).await
    }

    fn install_paths(&self) -> Vec<PathBuf> {
        vec![
            PathBuf::from("/opt/homebrew/Cellar"),
            PathBuf::from("/usr/local/Cellar"),
            PathBuf::from("/opt/homebrew/Caskroom"),
            PathBuf::from("/usr/local/Caskroom"),
            home_path("~/Library/Caches/Homebrew/downloads"),
            PathBuf::from("/opt/homebrew/Library/Taps"),
            PathBuf::from("/usr/local/Homebrew/Library/Taps"),
        ]
    }

    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}

fn installed_versions(output: &str) -> impl Iterator<Item = (String, Option<String>)> + '_ {
    lines(output).map(|line| {
        let mut fields = line.split_whitespace();
        let name = fields.next().unwrap_or_default().to_owned();
        let version = fields.next().map(str::to_owned);
        (name, version)
    })
}

fn changed_taps(output: &str) -> std::collections::HashSet<&str> {
    let Some(start) = output.find('(') else {
        return std::collections::HashSet::new();
    };
    let Some(end) = output[start + 1..].find(')') else {
        return std::collections::HashSet::new();
    };
    output[start + 1..start + 1 + end]
        .split(|character| character == ',' || character == '\n')
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|part| part.contains('/') && !part.contains(' '))
        .collect()
}
