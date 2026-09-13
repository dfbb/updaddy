use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    askpass_path, classify_process_error, command, detect_process_error, home_path, package_record,
    validate_name, validate_resource_name, EcosystemAdapter, ExecutorContext,
};
use super::parsers::lines;

const COMPATIBLE_TARGETS_SCRIPT: &str = r##"
require 'rubygems/spec_fetcher'

def compatible_with_runtime?(spec)
  spec.required_ruby_version.satisfied_by?(Gem.ruby_version) &&
    spec.required_rubygems_version.satisfied_by?(Gem.rubygems_version)
end

if ARGV == ['--self-test']
  exit compatible_with_runtime?(Gem::Specification.new) ? 0 : 1
end

fetcher = Gem::SpecFetcher.fetcher
ARGV.each_slice(2) do |name, version|
  dependency = Gem::Dependency.new(name, "= #{version}")
  specs, errors = fetcher.spec_for_dependency(dependency)
  raise errors.first.exception if specs.empty? && errors.first.respond_to?(:exception)
  compatible = specs.map(&:first).any? { |spec| compatible_with_runtime?(spec) }
  puts "#{name}\t#{version}" if compatible
end
"##;

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
            .run_direct(command("gem", ["--version"]), CancellationToken::new())
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
            .run(command("gem", ["outdated"]), cancel.clone())
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
            if parsed_updates.is_empty()
                || classified.is_retryable()
                || matches!(
                    classified,
                    TaskErrorKind::PermissionDenied | TaskErrorKind::InvalidInput
                )
            {
                return Err(classified);
            }
        }
        let compatible_targets = self
            .compatible_targets(context, &parsed_updates, cancel)
            .await?;
        let updates = parsed_updates
            .into_iter()
            .filter(|(name, _, target)| {
                compatible_targets.contains(&(name.clone(), target.clone()))
            })
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
                    if expected.as_deref().is_none_or(|current| current == version) {
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

    async fn execute(
        &self,
        context: &ExecutorContext,
        task: &PackageTask,
        cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        let mut spec = self.plan(task)?;
        if task.operation == Operation::Update {
            let name = task
                .name
                .split_once('@')
                .map_or(task.name.as_str(), |(name, _)| name);
            let outdated = context
                .run(command("gem", ["outdated", name]), cancel.clone())
                .await
                .map_err(|error| classify_process_error(&error))?;
            if !outdated.status.success() && outdated.status.code() != Some(1) {
                return Err(self.classify_error(&outdated));
            }
            let candidates = lines(&outdated.stdout)
                .filter_map(parse_outdated_line)
                .collect::<Vec<_>>();
            let compatible = self
                .compatible_targets(context, &candidates, cancel.clone())
                .await?;
            let target = candidates
                .iter()
                .rev()
                .find_map(|(candidate_name, _, target)| {
                    (candidate_name == name
                        && compatible.contains(&(candidate_name.clone(), target.clone())))
                    .then_some(target.as_str())
                })
                .ok_or(TaskErrorKind::InvalidInput)?;
            // `gem update name` always resolves the newest release and can select
            // one requiring a newer Ruby. Pin the compatible release explicitly.
            spec = command(
                "gem",
                ["install", name, "--version", target, "--no-document"],
            );
        }
        let gem_home = self
            .discover_primary_install_path(context, cancel.clone())
            .await?;
        if !path_or_parent_writable(&gem_home) {
            let executable = context
                .run_direct(command("/usr/bin/which", ["gem"]), cancel.clone())
                .await
                .map_err(|error| classify_process_error(&error))?;
            let executable = PathBuf::from(executable.stdout.trim());
            if !executable.is_absolute() || !executable.is_file() {
                return Err(TaskErrorKind::CommandFailed);
            }
            let askpass = askpass_path().ok_or(TaskErrorKind::CommandFailed)?;
            spec.program = executable.to_string_lossy().into_owned();
            spec.sudo = true;
            spec.env.insert(
                "SUDO_ASKPASS".into(),
                askpass.to_string_lossy().into_owned(),
            );
        }
        let result = context
            .run(spec, cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if result.status.success() && !has_gem_failure_output(&result) {
            Ok(())
        } else {
            Err(self.classify_error(&result))
        }
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
    ) -> Result<crate::disk_usage::PackageInstallPaths, TaskErrorKind> {
        let (name, version) = package.name.rsplit_once('@').unwrap_or((
            package.name.as_str(),
            package.current_version.as_deref().unwrap_or(""),
        ));
        validate_resource_name(Ecosystem::Gem, ResourceKind::Package, name)?;
        validate_name(version)?;
        let result = context
            .run_direct(
                command("gem", ["contents", name, "--version", version]),
                cancel,
            )
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        Ok(gem_content_paths(
            name,
            version,
            install_roots,
            &result.stdout,
        ))
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

fn path_or_parent_writable(path: &Path) -> bool {
    let Some(existing) = path.ancestors().find(|candidate| candidate.exists()) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        CString::new(existing.as_os_str().as_bytes())
            .ok()
            .is_some_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) } == 0)
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(existing).is_ok_and(|metadata| !metadata.permissions().readonly())
    }
}

fn has_gem_failure_output(result: &CommandResult) -> bool {
    let output = format!("{}\n{}", result.stdout, result.stderr).to_ascii_lowercase();
    [
        "error installing",
        "failed to build gem native extension",
        "there are no versions of",
        "requires ruby version",
        "gem::filepermissionerror",
    ]
    .iter()
    .any(|marker| output.contains(marker))
}

impl GemAdapter {
    async fn discover_primary_install_path(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<PathBuf, TaskErrorKind> {
        let result = context
            .run_direct(command("gem", ["env", "gemdir"]), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        let path = result.stdout.trim();
        if path.is_empty() {
            return Err(TaskErrorKind::CommandFailed);
        }
        Ok(home_path(path))
    }

    async fn compatible_targets(
        &self,
        context: &ExecutorContext,
        updates: &[(String, Option<String>, String)],
        cancel: CancellationToken,
    ) -> Result<std::collections::HashSet<(String, String)>, TaskErrorKind> {
        if updates.is_empty() {
            return Ok(std::collections::HashSet::new());
        }
        let mut args = vec!["-e".to_owned(), COMPATIBLE_TARGETS_SCRIPT.to_owned()];
        for (name, _, target) in updates {
            args.push(name.clone());
            args.push(target.clone());
        }
        let result = context
            .run(command("ruby", args), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        Ok(parse_compatible_targets(&result.stdout))
    }

    /// Resolve every directory searched by the active RubyGems environment.
    async fn discover_install_paths_with_cancel(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        let result = context
            .run_direct(command("gem", ["env", "gempath"]), cancel)
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        let paths = std::env::split_paths(std::ffi::OsStr::new(result.stdout.trim()))
            .map(home_path)
            .collect::<Vec<_>>();
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

fn gem_content_paths(
    name: &str,
    version: &str,
    roots: &[PathBuf],
    output: &str,
) -> crate::disk_usage::PackageInstallPaths {
    let directory = format!("{name}-{version}");
    let install_root = roots
        .iter()
        .find(|root| {
            root.join("gems").join(&directory).exists()
                || root
                    .join("specifications")
                    .join(format!("{directory}.gemspec"))
                    .exists()
                || root
                    .join("specifications/default")
                    .join(format!("{directory}.gemspec"))
                    .exists()
        })
        .cloned()
        .or_else(|| roots.first().cloned())
        .unwrap_or_default();
    let mut paths = lines(output)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.exists())
        .collect::<Vec<_>>();
    for specification in [
        install_root
            .join("specifications")
            .join(format!("{directory}.gemspec")),
        install_root
            .join("specifications/default")
            .join(format!("{directory}.gemspec")),
    ] {
        if specification.exists() {
            paths.push(specification);
        }
    }
    paths.sort();
    paths.dedup();
    crate::disk_usage::PackageInstallPaths {
        measurable: !paths.is_empty(),
        install_root,
        paths,
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
    // RubyGems also emits `name (current, newest)` on some versions.
    if let Some((current, target)) = body.split_once(',') {
        let current = current
            .trim()
            .strip_prefix("current ")
            .or_else(|| current.trim().strip_prefix("installed "))
            .unwrap_or(current.trim());
        let target = target
            .trim()
            .strip_prefix("newest ")
            .unwrap_or(target.trim());
        return (!name.is_empty() && !current.is_empty() && !target.is_empty())
            .then(|| (name.to_owned(), Some(current.to_owned()), target.to_owned()));
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

fn parse_compatible_targets(output: &str) -> std::collections::HashSet<(String, String)> {
    lines(output)
        .filter_map(|line| line.split_once('\t'))
        .filter(|(name, version)| !name.is_empty() && !version.is_empty())
        .map(|(name, version)| (name.to_owned(), version.to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        gem_content_paths, has_gem_failure_output, parse_compatible_targets, parse_outdated_line,
        GemAdapter,
    };
    use crate::adapters::{CommandRunner, EcosystemAdapter, ExecutorContext};
    use crate::core::{Ecosystem, Operation, PackageTask};
    use crate::executor::{CommandResult, CommandSpec, ProcessError};
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    struct GemPermissionRunner {
        executable: PathBuf,
        specs: Mutex<Vec<CommandSpec>>,
    }

    #[async_trait]
    impl CommandRunner for GemPermissionRunner {
        async fn run(
            &self,
            spec: CommandSpec,
            _cancel: CancellationToken,
        ) -> Result<CommandResult, ProcessError> {
            let stdout = if spec.args == ["env", "gemdir"] {
                "/Library/Ruby/Gems/2.6.0\n".to_owned()
            } else if spec.program == "/usr/bin/which" {
                format!("{}\n", self.executable.display())
            } else if spec.args == ["outdated", "rake"] {
                "rake (12.3.3 < 13.0.6)\n".to_owned()
            } else if spec.program == "ruby" {
                "rake\t13.0.6\n".to_owned()
            } else {
                String::new()
            };
            self.specs.lock().unwrap().push(spec);
            Ok(CommandResult {
                status: Command::new("sh").arg("-c").arg("exit 0").status().unwrap(),
                stdout,
                stderr: String::new(),
                duration: Duration::ZERO,
            })
        }
    }

    #[test]
    fn parses_rubygems_current_and_target_versions() {
        assert_eq!(
            parse_outdated_line("rails (7.1.0 < 7.2.0)"),
            Some(("rails".into(), Some("7.1.0".into()), "7.2.0".into()))
        );
        assert_eq!(parse_outdated_line("rails (current <)"), None);
    }

    #[test]
    fn treats_gem_error_output_as_failure_even_with_zero_exit_status() {
        let result = CommandResult {
            status: Command::new("sh").arg("-c").arg("exit 0").status().unwrap(),
            stdout: String::new(),
            stderr: "ERROR: Failed to build gem native extension".into(),
            duration: Duration::ZERO,
        };

        assert!(has_gem_failure_output(&result));
    }

    #[test]
    fn parses_only_compatible_gem_targets() {
        let targets = parse_compatible_targets("rake\t13.2.1\nopenssl\t\ninvalid\n");
        assert!(targets.contains(&("rake".into(), "13.2.1".into())));
        assert!(!targets.iter().any(|(name, _)| name == "openssl"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn compatibility_script_runs_on_the_system_ruby() {
        let status = Command::new("/usr/bin/ruby")
            .args(["-e", super::COMPATIBLE_TARGETS_SCRIPT, "--", "--self-test"])
            .status()
            .unwrap();

        assert!(status.success());
    }

    #[tokio::test]
    async fn update_checks_gemdir_instead_of_the_first_gempath() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("gem");
        std::fs::write(&executable, "fake gem").unwrap();
        let runner = Arc::new(GemPermissionRunner {
            executable: executable.clone(),
            specs: Mutex::new(Vec::new()),
        });
        let context = ExecutorContext::with_runner(runner.clone());
        let task = PackageTask::new(Ecosystem::Gem, "rake@12.3.3", Operation::Update);

        GemAdapter::new()
            .execute(&context, &task, CancellationToken::new())
            .await
            .unwrap();

        let specs = runner.specs.lock().unwrap();
        assert!(specs.iter().any(|spec| spec.args == ["env", "gemdir"]));
        assert!(specs.last().unwrap().sudo);
        assert_eq!(specs.last().unwrap().program, executable.to_string_lossy());
        assert_eq!(
            specs.last().unwrap().args,
            ["install", "rake", "--version", "13.0.6", "--no-document"]
        );
    }

    #[test]
    fn resolves_user_and_default_gem_content_paths() {
        let directory = tempfile::tempdir().unwrap();
        let system = directory.path().join("system");
        let user = directory.path().join("user");
        let user_gem = user.join("gems/demo-2.0.0");
        std::fs::create_dir_all(&user_gem).unwrap();
        std::fs::write(user_gem.join("demo.rb"), "demo").unwrap();
        let paths = gem_content_paths(
            "demo",
            "2.0.0",
            &[system.clone(), user.clone()],
            &format!("{}\n", user_gem.join("demo.rb").display()),
        );
        assert!(paths.measurable);
        assert_eq!(paths.install_root, user);

        let default_spec = system.join("specifications/default/openssl-2.1.2.gemspec");
        std::fs::create_dir_all(default_spec.parent().unwrap()).unwrap();
        std::fs::write(&default_spec, "spec").unwrap();
        let stdlib = directory.path().join("ruby/openssl.rb");
        std::fs::create_dir_all(stdlib.parent().unwrap()).unwrap();
        std::fs::write(&stdlib, "openssl").unwrap();
        let paths = gem_content_paths(
            "openssl",
            "2.1.2",
            &[system.clone(), directory.path().join("framework")],
            &format!("{}\n", stdlib.display()),
        );
        assert!(paths.paths.contains(&stdlib));
        assert!(paths.paths.contains(&default_spec));
        assert_eq!(paths.install_root, system);
    }
}
