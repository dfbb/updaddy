use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    classify_process_error, command, detect_process_error, home_path, package_record,
    validate_name, validate_resource_name, EcosystemAdapter, ExecutorContext,
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
            Err(error) => detect_process_error(&error),
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
            .map_err(|error| classify_process_error(&error))?;
        if !listed.status.success() {
            return Err(self.classify_error(&listed));
        }
        let outdated = context
            .run(command("gem", ["outdated"]), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !outdated.status.success() && outdated.status.code() != Some(1) {
            return Err(self.classify_error(&outdated));
        }
        let parsed_updates = lines(&outdated.stdout)
            .filter_map(parse_outdated_line)
            .collect::<Vec<_>>();
        if !outdated.status.success() {
            let classified = self.classify_error(&outdated);
            if classified.is_retryable()
                || matches!(
                    classified,
                    TaskErrorKind::PermissionDenied | TaskErrorKind::InvalidInput
                )
                || parsed_updates.is_empty()
            {
                return Err(classified);
            }
        }
        let updates = parsed_updates
            .into_iter()
            .map(|(name, current, target)| (name, (current, target)))
            .collect::<std::collections::HashMap<_, _>>();
        let mut records = Vec::new();
        for line in lines(&listed.stdout) {
            let Some((name, versions)) = line.split_once(" (") else {
                continue;
            };
            let versions = versions
                .trim_end_matches(')')
                .split(',')
                .map(str::trim)
                .map(|version| version.strip_prefix("default: ").unwrap_or(version))
                .filter(|version| !version.is_empty());
            for version in versions {
                let target = updates.get(name).and_then(|(expected, target)| {
                    if expected
                        .as_deref()
                        .map_or(true, |current| current == version)
                    {
                        Some(target.clone())
                    } else {
                        None
                    }
                });
                // Include the selected version in the task identity so uninstall never
                // falls back to RubyGems' interactive multi-version prompt.
                records.push(package_record(
                    Ecosystem::Gem,
                    ResourceKind::Package,
                    format!("{name}@{version}"),
                    Some(version.to_owned()),
                    target,
                ));
            }
        }
        Ok(records)
    }

    async fn resolve_install_paths(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        self.discover_install_paths_with_cancel(context, cancel)
            .await
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
        validate_resource_name(Ecosystem::Gem, ResourceKind::Package, name)?;
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
    async fn discover_install_paths_with_cancel(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        let result = context
            .run(command("gem", ["env", "gemdir"]), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        let paths = lines(&result.stdout).map(home_path).collect::<Vec<_>>();
        if paths.is_empty() {
            return Err(TaskErrorKind::CommandFailed);
        }
        Ok(paths)
    }

    pub async fn discover_install_paths(
        &self,
        context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        self.discover_install_paths_with_cancel(context, CancellationToken::new())
            .await
    }
}

fn parse_outdated_line(line: &str) -> Option<(String, Option<String>, String)> {
    let (name, rest) = line.split_once(" (")?;
    let body = rest.strip_suffix(')')?.trim();
    if let Some((current, target)) = body.split_once('<') {
        let current = current
            .trim()
            .strip_prefix("current ")
            .or_else(|| current.trim().strip_prefix("installed "))
            .unwrap_or(current.trim());
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        let current = (!current.is_empty()).then(|| current.to_owned());
        return Some((name.to_owned(), current, target.to_owned()));
    }
    let newest = body.strip_prefix("newest ")?;
    let target = newest.split(',').next().map(str::trim).unwrap_or_default();
    let current = newest.split(',').find_map(|part| {
        part.trim()
            .strip_prefix("installed ")
            .or_else(|| part.trim().strip_prefix("current "))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    });
    (!name.is_empty() && !target.is_empty()).then(|| (name.to_owned(), current, target.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::parse_outdated_line;

    #[test]
    fn parses_rubygems_current_and_target_versions() {
        assert_eq!(
            parse_outdated_line("rails (7.1.0 < 7.2.0)"),
            Some(("rails".into(), Some("7.1.0".into()), "7.2.0".into()))
        );
        assert_eq!(parse_outdated_line("rails (current <)"), None);
    }
}
