use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{command, package_record, validate_name, EcosystemAdapter, ExecutorContext};
use super::parsers::lines;

#[derive(Debug, Clone, Default)]
pub struct GemAdapter;
impl GemAdapter {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EcosystemAdapter for GemAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Gem
    }
    async fn detect(&self, context: &ExecutorContext) -> Result<bool, TaskErrorKind> {
        match context
            .run(command("gem", ["--version"]), CancellationToken::new())
            .await
        {
            Ok(result) if result.status.success() => Ok(true),
            Ok(result) => Err(self.classify_error(&result)),
            Err(_) => Ok(false),
        }
    }
    async fn scan(&self, context: &ExecutorContext) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        let listed = context
            .run(
                command("gem", ["list", "--local"]),
                CancellationToken::new(),
            )
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !listed.status.success() {
            return Err(self.classify_error(&listed));
        }
        let outdated = context
            .run(command("gem", ["outdated"]), CancellationToken::new())
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !outdated.status.success() && !outdated.stdout.is_empty() {
            return Err(self.classify_error(&outdated));
        }
        let mut updates = std::collections::HashMap::new();
        for line in lines(&outdated.stdout) {
            if let Some((name, rest)) = line.split_once(" (") {
                let target = rest
                    .split("newest ")
                    .nth(1)
                    .and_then(|part| part.split(|c| c == ',' || c == ')').next())
                    .map(str::to_owned);
                updates.insert(name.to_owned(), target);
            }
        }
        let mut records = Vec::new();
        for line in lines(&listed.stdout) {
            let Some((name, versions)) = line.split_once(" (") else {
                continue;
            };
            let current = versions
                .trim_end_matches(')')
                .split(',')
                .next()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            records.push(package_record(
                Ecosystem::Gem,
                ResourceKind::Package,
                name,
                current,
                updates.get(name).cloned().flatten(),
            ));
        }
        Ok(records)
    }
    fn plan(&self, task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        if task.ecosystem != Ecosystem::Gem
            || !matches!(task.operation, Operation::Update | Operation::Uninstall)
        {
            return Err(TaskErrorKind::InvalidInput);
        }
        let (name, version) = task
            .name
            .split_once('@')
            .map_or((task.name.as_str(), None), |(n, v)| (n, Some(v)));
        validate_name(name)?;
        let args: Vec<String> = if matches!(task.operation, Operation::Update) {
            vec!["update".into(), name.into()]
        } else {
            let mut args = vec!["uninstall".into(), name.into()];
            if let Some(version) = version {
                validate_name(version)?;
                args.extend(["--version".into(), version.into()]);
            }
            args
        };
        Ok(command("gem", args))
    }
    fn install_paths(&self) -> Vec<PathBuf> {
        vec![PathBuf::from("gemdir")]
    }
    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}

impl GemAdapter {
    /// Resolve the active Ruby gem directory without consulting Bundler or a project Gemfile.
    pub async fn discover_install_paths(
        &self,
        context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        let result = context
            .run(command("gem", ["env", "gemdir"]), CancellationToken::new())
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        let mut paths = lines(&result.stdout).map(PathBuf::from).collect::<Vec<_>>();
        if paths.is_empty() {
            return Err(TaskErrorKind::CommandFailed);
        }
        // A gemdir is the root; gemspec files are included for disk accounting by callers.
        paths.push(paths[0].join("specifications"));
        Ok(paths)
    }
}
