use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    command, home_path, package_record, validate_name, EcosystemAdapter, ExecutorContext,
};
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
            .run(command("gem", ["--version"]), CancellationToken::new())
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
        let listed = context
            .run(command("gem", ["list", "--local"]), cancel.clone())
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !listed.status.success() {
            return Err(self.classify_error(&listed));
        }
        let outdated = context
            .run(command("gem", ["outdated"]), cancel)
            .await
            .map_err(|_| TaskErrorKind::CommandFailed)?;
        if !outdated.status.success() && outdated.status.code() != Some(1) {
            return Err(self.classify_error(&outdated));
        }
        if !outdated.status.success()
            && !lines(&outdated.stdout).any(|line| {
                line.contains(" (")
                    && (line.contains("newest ")
                        || line.contains("current ")
                        || line.contains(" < ")
                        || line.contains("<"))
            })
        {
            return Err(self.classify_error(&outdated));
        }
        let mut updates = std::collections::HashMap::new();
        for line in lines(&outdated.stdout) {
            if let Some((name, rest)) = line.split_once(" (") {
                let target = rest
                    .split("newest ")
                    .nth(1)
                    .or_else(|| rest.split('<').nth(1))
                    .and_then(|part| part.split(|c| c == ',' || c == ')').next())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                let current = rest
                    .split("installed ")
                    .nth(1)
                    .or_else(|| rest.split("current ").nth(1))
                    .or_else(|| rest.split('(').nth(1))
                    .and_then(|part| part.split(|c| c == ',' || c == ')' || c == '<').next())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                updates.insert(name.to_owned(), (current, target));
            }
        }
        let mut records = Vec::new();
        for line in lines(&listed.stdout) {
            let Some((name, versions)) = line.split_once(" (") else {
                continue;
            };
            let listed_current = versions
                .trim_end_matches(')')
                .split(',')
                .next()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned);
            let (update_current, target) = updates
                .get(name)
                .cloned()
                .unwrap_or((listed_current.clone(), None));
            records.push(package_record(
                Ecosystem::Gem,
                ResourceKind::Package,
                name,
                update_current.or(listed_current),
                target,
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
        vec![
            home_path("~/.gem"),
            PathBuf::from("/Library/Ruby/Gems"),
            PathBuf::from("/usr/local/lib/ruby/gems"),
        ]
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
        // Include the specifications directory and its gemspec files for disk accounting.
        let specifications = paths[0].join("specifications");
        paths.push(specifications.clone());
        if let Ok(entries) = std::fs::read_dir(&specifications) {
            paths.extend(
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "gemspec")),
            );
        }
        Ok(paths)
    }
}
