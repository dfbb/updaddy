use std::path::PathBuf;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::core::{Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind};
use crate::disk_usage::normalize_python_name;
use crate::executor::{CommandResult, CommandSpec};

use super::adapter::{
    classify_process_error, command, detect_process_error, home_path, package_record,
    validate_resource_name, EcosystemAdapter, ExecutorContext,
};
use super::parsers::json_array_versions;

#[derive(Debug, Clone, Default)]
pub struct PipAdapter;
impl PipAdapter {
    pub fn new() -> Self {
        Self
    }

    async fn discover_install_paths_with_cancel(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        let result = context
            .run_direct(
                command(
                    "python3",
                    [
                        "-c",
                        "import site; print('\\n'.join([*site.getsitepackages(), site.getusersitepackages()]))",
                    ],
                ),
                cancel.clone(),
            )
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !result.status.success() {
            return Err(self.classify_error(&result));
        }
        let paths: Vec<PathBuf> = result
            .stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(home_path)
            .collect();
        if paths.is_empty() {
            Err(TaskErrorKind::CommandFailed)
        } else {
            Ok(paths)
        }
    }

    pub async fn discover_install_paths(
        &self,
        context: &ExecutorContext,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        self.discover_install_paths_with_cancel(context, CancellationToken::new())
            .await
    }
}

#[async_trait]
impl EcosystemAdapter for PipAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Pip
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
            .run_direct(
                command("python3", ["-m", "pip", "--version"]),
                CancellationToken::new(),
            )
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
        let installed = context
            .run(
                command("python3", ["-m", "pip", "list", "--format=json"]),
                cancel.clone(),
            )
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !installed.status.success() {
            return Err(self.classify_error(&installed));
        }
        let outdated = context
            .run(
                command(
                    "python3",
                    ["-m", "pip", "list", "--outdated", "--format=json"],
                ),
                cancel.clone(),
            )
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !outdated.status.success() {
            return Err(self.classify_error(&outdated));
        }
        let manageable = context
            .run(
                command(
                    "python3",
                    [
                        "-c",
                        "import importlib.metadata as m,json; print(json.dumps([(d.metadata.get('Name'), d.version) for d in m.distributions() if d.metadata.get('Name') and d.read_text('RECORD') is not None]))",
                    ],
                ),
                cancel,
            )
            .await
            .map_err(|error| classify_process_error(&error))?;
        if !manageable.status.success() {
            return Err(self.classify_error(&manageable));
        }
        let manageable = manageable_distributions(&manageable.stdout)?;
        let updates: std::collections::HashMap<_, _> = json_array_versions(&outdated.stdout)
            .map_err(|_| TaskErrorKind::CommandFailed)?
            .into_iter()
            .map(|(n, _, target)| (n, target))
            .collect();
        Ok(json_array_versions(&installed.stdout)
            .map_err(|_| TaskErrorKind::CommandFailed)?
            .into_iter()
            .filter(|(name, version, _)| {
                manageable.contains(&(normalize_python_name(name), version.clone()))
            })
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

    async fn resolve_install_paths(
        &self,
        context: &ExecutorContext,
        cancel: CancellationToken,
    ) -> Result<Vec<PathBuf>, TaskErrorKind> {
        self.discover_install_paths_with_cancel(context, cancel)
            .await
    }

    fn plan(&self, task: &PackageTask) -> Result<CommandSpec, TaskErrorKind> {
        if task.ecosystem != Ecosystem::Pip
            || !matches!(task.operation, Operation::Update | Operation::Uninstall)
        {
            return Err(TaskErrorKind::InvalidInput);
        }
        validate_resource_name(Ecosystem::Pip, ResourceKind::Package, &task.name)?;
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
        Ok(command("python3", args))
    }
    fn install_paths(&self) -> Vec<PathBuf> {
        vec![
            home_path("~/.local/lib/python/site-packages"),
            PathBuf::from("/usr/local/lib/python3/site-packages"),
            PathBuf::from("/opt/homebrew/lib/python3/site-packages"),
        ]
    }
    fn classify_error(&self, result: &CommandResult) -> TaskErrorKind {
        super::adapter::classify_command_error(result)
    }
}

fn manageable_distributions(
    output: &str,
) -> Result<std::collections::HashSet<(String, String)>, TaskErrorKind> {
    serde_json::from_str::<Vec<(String, String)>>(output)
        .map_err(|_| TaskErrorKind::CommandFailed)
        .map(|distributions| {
            distributions
                .into_iter()
                .map(|(name, version)| (normalize_python_name(&name), version))
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::manageable_distributions;

    #[test]
    fn manageable_distributions_require_the_exact_installed_version() {
        let distributions =
            manageable_distributions(r#"[["pydantic_core", "2.41.5"], ["demo.package", "1.0"]]"#)
                .unwrap();

        assert!(distributions.contains(&("pydantic-core".into(), "2.41.5".into())));
        assert!(!distributions.contains(&("pydantic-core".into(), "2.46.5".into())));
        assert!(distributions.contains(&("demo-package".into(), "1.0".into())));
    }
}
