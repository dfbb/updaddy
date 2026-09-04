use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::disk_usage::{resolve_package_paths, PackageInstallPaths};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    classify_process_error, command, detect_process_error, home_path, package_record,
    validate_resource_name, EcosystemAdapter, ExecutorContext,
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
                command("brew", ["outdated", "--formula", "--verbose"]),
                cancel.clone(),
            )
            .await?;
        let cask = self
            .run(
                context,
                command("brew", ["outdated", "--cask", "--verbose"]),
                cancel.clone(),
            )
            .await?;
        let taps = self.run(context, command("brew", ["tap"]), cancel).await?;
        let outdated_formula = outdated_versions(&formula.stdout);
        let outdated_cask = outdated_versions(&cask.stdout);
        let mut records = Vec::new();
        for (name, current) in installed_versions(&formula_installed.stdout) {
            let update = outdated_formula.get(&name);
            records.push(package_record(
                Ecosystem::Homebrew,
                ResourceKind::Formula,
                name,
                current,
                update.and_then(|(_, target)| target.clone()),
            ));
        }
        for (name, current) in installed_versions(&cask_installed.stdout) {
            let update = outdated_cask.get(&name);
            records.push(package_record(
                Ecosystem::Homebrew,
                ResourceKind::Cask,
                format!("cask:{name}"),
                current,
                update.and_then(|(_, target)| target.clone()),
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

    async fn update(
        &self,
        context: &ExecutorContext,
        task: &PackageTask,
        cancel: CancellationToken,
    ) -> Result<Vec<String>, TaskErrorKind> {
        let (kind, _) = Self::task_kind(&task.name);
        if kind != ResourceKind::Tap {
            self.execute(context, task, cancel).await?;
            return Ok(vec![task.name.clone()]);
        }
        let result = self.run(context, self.plan(task)?, cancel).await?;
        let output = format!("{}\n{}", result.stdout, result.stderr);
        let changed = changed_taps(&output)
            .into_iter()
            .map(|tap| format!("tap:{tap}"))
            .collect::<Vec<_>>();
        if changed.is_empty()
            && output.to_ascii_lowercase().contains("updated")
            && output.to_ascii_lowercase().contains("tap")
            && !output.to_ascii_lowercase().contains("already up-to-date")
        {
            return Err(TaskErrorKind::CommandFailed);
        }
        Ok(changed)
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

    async fn resolve_package_install_paths(
        &self,
        context: &ExecutorContext,
        package: &PackageRecord,
        install_roots: &[PathBuf],
        cancel: CancellationToken,
    ) -> Result<PackageInstallPaths, TaskErrorKind> {
        let fallback = resolve_package_paths(package, install_roots);
        if package.resource_kind != ResourceKind::Cask {
            return Ok(fallback);
        }
        let name = package.name.strip_prefix("cask:").unwrap_or(&package.name);
        validate_resource_name(Ecosystem::Homebrew, ResourceKind::Cask, name)?;
        let result = context
            .run(command("brew", ["list", "--cask", name]), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        Ok(resolve_cask_artifacts(fallback, &result.stdout))
    }

    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}

fn resolve_cask_artifacts(mut fallback: PackageInstallPaths, output: &str) -> PackageInstallPaths {
    for listed in lines(output)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        let resolved = std::fs::symlink_metadata(&listed)
            .ok()
            .filter(|metadata| metadata.file_type().is_symlink())
            .and_then(|_| std::fs::read_link(&listed).ok())
            .map(|target| {
                if target.is_absolute() {
                    target
                } else {
                    listed
                        .parent()
                        .map_or(target.clone(), |parent| parent.join(target))
                }
            })
            .unwrap_or(listed);
        if resolved.exists() && !fallback.paths.contains(&resolved) {
            fallback.paths.push(resolved);
        }
    }
    fallback.measurable = fallback.paths.iter().any(|path| path.exists());
    fallback
}

fn installed_versions(output: &str) -> impl Iterator<Item = (String, Option<String>)> + '_ {
    lines(output).map(|line| {
        let mut fields = line.split_whitespace();
        let name = fields.next().unwrap_or_default().to_owned();
        let version = fields.next().map(str::to_owned);
        (name, version)
    })
}

fn outdated_versions(
    output: &str,
) -> std::collections::HashMap<String, (Option<String>, Option<String>)> {
    lines(output)
        .filter_map(|line| {
            let name = line.split_whitespace().next()?.to_owned();
            let Some((_, rest)) = line.split_once(" (") else {
                return Some((name, (None, Some("latest".to_owned()))));
            };
            let close = rest.find(')')?;
            let body = rest[..close].trim();
            let after = rest[close + 1..].trim();
            let (current, target) = if let Some((current, target)) = body.split_once('<') {
                (
                    current.trim(),
                    target.split(" [").next().unwrap_or(target).trim(),
                )
            } else {
                let target = after
                    .strip_prefix("<")
                    .or_else(|| after.strip_prefix("!="))
                    .map(str::trim)?
                    .split(" [")
                    .next()
                    .unwrap_or_default()
                    .trim();
                (body.split(',').next().unwrap_or(body).trim(), target)
            };
            if target.is_empty() {
                return None;
            }
            Some((
                name,
                (
                    (!current.is_empty()).then(|| current.to_owned()),
                    Some(target.to_owned()),
                ),
            ))
        })
        .collect()
}

fn changed_taps(output: &str) -> std::collections::HashSet<String> {
    let mut changed = std::collections::HashSet::new();
    for line in output.lines().filter(|line| {
        let line = line.to_ascii_lowercase();
        line.contains("updated") && (line.contains(" tap ") || line.contains(" taps "))
    }) {
        let Some(start) = line.find('(') else {
            continue;
        };
        let Some(end) = line[start + 1..].find(')') else {
            continue;
        };
        changed.extend(
            line[start + 1..start + 1 + end]
                .split(',')
                .flat_map(|part| part.split(" and "))
                .map(str::trim)
                .filter(|part| part.contains('/') && !part.contains(' '))
                .map(str::to_owned),
        );
    }
    changed
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{changed_taps, outdated_versions, resolve_cask_artifacts};
    use crate::disk_usage::{measure_paths, PackageInstallPaths};

    #[test]
    fn parses_brew_verbose_formula_and_cask_rows() {
        let formula = outdated_versions("openssl (3.2.0) < 3.3.0");
        assert_eq!(
            formula.get("openssl"),
            Some(&(Some("3.2.0".into()), Some("3.3.0".into())))
        );
        let cask = outdated_versions("firefox (123.0) != 124.0");
        assert_eq!(
            cask.get("firefox"),
            Some(&(Some("123.0".into()), Some("124.0".into())))
        );
    }

    #[test]
    fn parses_only_taps_changed_by_this_brew_update() {
        let changed =
            changed_taps("Updated 1 tap (acme/one).\nNo changes for acme/two were requested.");
        assert_eq!(
            changed,
            std::collections::HashSet::from(["acme/one".into()])
        );
    }

    #[cfg(unix)]
    #[test]
    fn cask_artifact_resolution_counts_an_explicit_symlink_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let cask_root = directory.path().join("Caskroom");
        let package_root = cask_root.join("demo");
        let application = directory.path().join("Applications/Demo.app");
        fs::create_dir_all(&package_root).unwrap();
        fs::create_dir_all(&application).unwrap();
        fs::write(application.join("binary"), b"1234").unwrap();
        let artifact = package_root.join("Demo.app");
        symlink(&application, &artifact).unwrap();
        let fallback = PackageInstallPaths {
            install_root: cask_root,
            paths: vec![package_root],
            measurable: true,
        };

        let resolved =
            resolve_cask_artifacts(fallback, &format!("{}\n", artifact.to_string_lossy()));

        assert!(resolved.paths.contains(&application));
        assert_eq!(measure_paths(&resolved.paths).unwrap().bytes, 4);
    }
}
