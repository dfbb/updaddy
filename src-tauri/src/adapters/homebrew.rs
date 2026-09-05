use std::path::PathBuf;
use std::time::Instant;

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
        context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        self.discover_install_paths_with_cancel(context, CancellationToken::new())
            .await
    }

    async fn discover_install_paths_with_cancel(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        let result = context
            .run(command("brew", ["--prefix"]), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if result.status.success() {
            let prefix = PathBuf::from(result.stdout.trim());
            if !prefix.as_os_str().is_empty() {
                return Ok(vec![
                    prefix.join("Cellar"),
                    prefix.join("Caskroom"),
                    prefix.join("Library/Taps"),
                ]);
            }
        }
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
            .run_direct(command("brew", ["--version"]), CancellationToken::new())
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
                command("brew", ["info", "--cask", "--json=v2", "--installed"]),
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
        for (name, current) in installed_cask_versions(&cask_installed.stdout)? {
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
        let mut spec = command("brew", args);
        spec.pseudo_terminal = task.operation == crate::core::Operation::Update
            && matches!(kind, ResourceKind::Formula | ResourceKind::Cask);
        Ok(spec)
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
        if changed.is_empty() {
            Ok(vec![task.name.clone()])
        } else {
            Ok(changed)
        }
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

    async fn resolve_install_paths(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        self.discover_install_paths_with_cancel(context, cancel)
            .await
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

fn installed_cask_versions(output: &str) -> Result<Vec<(String, Option<String>)>, TaskErrorKind> {
    let root: serde_json::Value =
        serde_json::from_str(output).map_err(|_| TaskErrorKind::CommandFailed)?;
    let casks = root
        .get("casks")
        .and_then(serde_json::Value::as_array)
        .ok_or(TaskErrorKind::CommandFailed)?;
    casks
        .iter()
        .map(|cask| {
            let name = cask
                .get("token")
                .and_then(serde_json::Value::as_str)
                .ok_or(TaskErrorKind::CommandFailed)?;
            let version = cask
                .get("installed")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            Ok((name.to_owned(), version))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomebrewDownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub eta_seconds: Option<u64>,
}

#[derive(Default)]
pub(crate) struct HomebrewProgressTracker {
    buffer: String,
    last_sample: Option<(u64, Instant)>,
    bytes_per_second: Option<f64>,
    last_emitted: Option<(u64, u64)>,
}

impl HomebrewProgressTracker {
    pub fn push(&mut self, chunk: &[u8], now: Instant) -> Option<HomebrewDownloadProgress> {
        append_terminal_text(&mut self.buffer, chunk);
        if self.buffer.len() > 8_192 {
            self.buffer.drain(..self.buffer.len() - 4_096);
        }
        let (downloaded_bytes, total_bytes) = parse_latest_size_pair(&self.buffer)?;
        if self.last_emitted == Some((downloaded_bytes, total_bytes)) {
            return None;
        }

        if let Some((previous_bytes, previous_at)) = self.last_sample {
            if downloaded_bytes > previous_bytes {
                let elapsed = now.saturating_duration_since(previous_at).as_secs_f64();
                if elapsed >= 0.01 {
                    let current = (downloaded_bytes - previous_bytes) as f64 / elapsed;
                    self.bytes_per_second = Some(
                        self.bytes_per_second
                            .map_or(current, |previous| previous * 0.7 + current * 0.3),
                    );
                }
            } else if downloaded_bytes < previous_bytes {
                self.bytes_per_second = None;
            }
        }
        self.last_sample = Some((downloaded_bytes, now));
        self.last_emitted = Some((downloaded_bytes, total_bytes));

        let eta_seconds = self.bytes_per_second.and_then(|speed| {
            (speed > 1.0 && downloaded_bytes < total_bytes)
                .then(|| ((total_bytes - downloaded_bytes) as f64 / speed).ceil() as u64)
        });
        Some(HomebrewDownloadProgress {
            downloaded_bytes,
            total_bytes,
            eta_seconds,
        })
    }
}

fn append_terminal_text(output: &mut String, chunk: &[u8]) {
    let mut index = 0;
    while index < chunk.len() {
        match chunk[index] {
            0x1b => {
                index += 1;
                if chunk.get(index) == Some(&b'[') {
                    index += 1;
                    while index < chunk.len() {
                        let byte = chunk[index];
                        index += 1;
                        if (0x40..=0x7e).contains(&byte) {
                            break;
                        }
                    }
                }
            }
            b'\r' => {
                output.push('\n');
                index += 1;
            }
            byte if byte == b'\n' || byte.is_ascii_graphic() || byte == b' ' => {
                output.push(byte as char);
                index += 1;
            }
            _ => index += 1,
        }
    }
}

fn parse_latest_size_pair(output: &str) -> Option<(u64, u64)> {
    output.match_indices('/').rev().find_map(|(slash, _)| {
        let left = &output[..slash];
        let right = &output[slash + 1..];
        let downloaded = (left.len().saturating_sub(24)..left.len()).find_map(|start| {
            let candidate = left[start..].trim();
            parse_size_prefix(candidate)
                .filter(|(_, consumed)| *consumed == candidate.len())
                .map(|(bytes, _)| bytes)
        })?;
        let (total, _) = parse_size_prefix(right)?;
        (total > 0 && downloaded <= total).then_some((downloaded, total))
    })
}

fn parse_size_prefix(value: &str) -> Option<(u64, usize)> {
    let bytes = value.as_bytes();
    let mut index = bytes.iter().position(|byte| !byte.is_ascii_whitespace())?;
    let number_start = index;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
    {
        index += 1;
    }
    if index == number_start {
        return None;
    }
    let number = value[number_start..index].parse::<f64>().ok()?;
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let unit_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
        index += 1;
    }
    let multiplier = match value[unit_start..index].to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "KB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "TB" => 1_000_000_000_000.0,
        _ => return None,
    };
    Some(((number * multiplier).round() as u64, index))
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
    use std::time::{Duration, Instant};

    use super::{
        changed_taps, installed_cask_versions, outdated_versions, resolve_cask_artifacts,
        HomebrewProgressTracker,
    };
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
    fn parses_installed_casks_without_loading_untrusted_tap_definitions() {
        let casks = installed_cask_versions(
            r#"{"formulae":[],"casks":[{"token":"firefox","installed":"124.0"},{"token":"depotdownloader","installed":"3.4.0"}]}"#,
        )
        .unwrap();

        assert_eq!(
            casks,
            vec![
                ("firefox".into(), Some("124.0".into())),
                ("depotdownloader".into(), Some("3.4.0".into())),
            ]
        );
    }

    #[test]
    fn parses_homebrew_tty_download_progress_and_estimates_remaining_time() {
        let started = Instant::now();
        let mut tracker = HomebrewProgressTracker::default();
        let first = tracker
            .push(b"chatgpt  Downloading  10.0MB/100.0MB", started)
            .unwrap();
        assert_eq!(first.downloaded_bytes, 10_000_000);
        assert_eq!(first.total_bytes, 100_000_000);
        assert_eq!(first.eta_seconds, None);

        let second = tracker
            .push(
                b"\x1b[0Gchatgpt  Downloading  30.0MB/100.0MB",
                started + Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(second.downloaded_bytes, 30_000_000);
        assert_eq!(second.total_bytes, 100_000_000);
        assert_eq!(second.eta_seconds, Some(7));
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
