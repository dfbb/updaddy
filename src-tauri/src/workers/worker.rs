use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::adapters::ExecutorContext;
use crate::core::{
    DiskUsage, DiskUsageStatus, Ecosystem, Operation, PackageRecord, PackageTask, TaskErrorKind,
    TaskStatus,
};
use crate::disk_usage::{CacheStore, DiskUsageError, DiskUsageService, PackageInstallPaths};
use crate::persistence::Database;
use crate::proxy::ProxyError;

use super::messages::WorkerCommand;
use super::{WorkerEvent, WorkerEventSink};

pub use crate::adapters::EcosystemAdapter;

/// 将代理运行时错误映射到现有 worker 重试语义；配置错误保持不可重试。
pub(crate) fn classify_proxy_error(error: ProxyError) -> TaskErrorKind {
    match error {
        ProxyError::InvalidConfig | ProxyError::ProxyUnsupported => TaskErrorKind::InvalidInput,
        ProxyError::ProxyUnavailable | ProxyError::BridgeFailed => TaskErrorKind::ProxyDisconnected,
    }
}

/// Adapter used when a worker has no implementation yet.
pub struct NoopAdapter;

impl EcosystemAdapter for NoopAdapter {}

const RETRY_DELAYS_SECONDS: [u64; 3] = [5, 30, 120];
const MAX_ATTEMPTS: u32 = 1 + RETRY_DELAYS_SECONDS.len() as u32;
const MAX_DISK_MEASUREMENTS: usize = 4;

struct DiskMeasurementJob {
    task: PackageTask,
    package: PackageRecord,
    paths: PackageInstallPaths,
    cached: Option<crate::persistence::DiskUsageCacheEntry>,
}

async fn run_with_retries(
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    command: WorkerCommand,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    task_id: Uuid,
) -> Result<(), TaskErrorKind> {
    let mut attempt = 1_u32;
    loop {
        let started_at = Utc::now().timestamp();
        let result = adapter
            .run_with_context(executor, command.clone(), cancel.clone())
            .await;
        let status = match &result {
            Ok(()) => "succeeded",
            Err(_) if cancel.is_cancelled() => "cancelled",
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => "retrying",
            Err(_) => "failed",
        };
        if let Some(database) = database {
            let _ = database.record_task_attempt(
                task_id,
                attempt,
                status,
                Some(started_at),
                Some(Utc::now().timestamp()),
            );
        }
        match result {
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                let delay = Duration::from_secs(RETRY_DELAYS_SECONDS[(attempt - 1) as usize]);
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => return Err(TaskErrorKind::CommandFailed),
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            result => return result,
        }
    }
}

async fn update_with_retries(
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    task: &PackageTask,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    task_id: Uuid,
) -> Result<Vec<String>, TaskErrorKind> {
    let mut attempt = 1_u32;
    loop {
        let started_at = Utc::now().timestamp();
        let result = adapter.update(executor, task, cancel.clone()).await;
        let status = match &result {
            Ok(_) => "succeeded",
            Err(_) if cancel.is_cancelled() => "cancelled",
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => "retrying",
            Err(_) => "failed",
        };
        if let Some(database) = database {
            let _ = database.record_task_attempt(
                task_id,
                attempt,
                status,
                Some(started_at),
                Some(Utc::now().timestamp()),
            );
        }
        match result {
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                let delay = Duration::from_secs(RETRY_DELAYS_SECONDS[(attempt - 1) as usize]);
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => return Err(TaskErrorKind::CommandFailed),
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            result => return result,
        }
    }
}

async fn scan_with_retries(
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    ecosystem: Ecosystem,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    task_id: Uuid,
) -> Result<Vec<PackageRecord>, TaskErrorKind> {
    let mut attempt = 1_u32;
    loop {
        let started_at = Utc::now().timestamp();
        let result = match adapter.scan_with_cancel(executor, cancel.clone()).await {
            Err(TaskErrorKind::Unknown) => adapter
                .run_with_context(executor, WorkerCommand::Scan(ecosystem), cancel.clone())
                .await
                .map(|()| Vec::new()),
            result => result,
        };
        let status = match &result {
            Ok(_) => "succeeded",
            Err(_) if cancel.is_cancelled() => "cancelled",
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => "retrying",
            Err(_) => "failed",
        };
        if let Some(database) = database {
            let _ = database.record_task_attempt(
                task_id,
                attempt,
                status,
                Some(started_at),
                Some(Utc::now().timestamp()),
            );
        }
        match result {
            Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                let delay = Duration::from_secs(RETRY_DELAYS_SECONDS[(attempt - 1) as usize]);
                attempt += 1;
                tokio::select! {
                    _ = cancel.cancelled() => return Err(TaskErrorKind::CommandFailed),
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            result => return result,
        }
    }
}

pub(crate) async fn run_command(
    ecosystem: Ecosystem,
    task_id: Uuid,
    command: WorkerCommand,
    adapter: Arc<dyn EcosystemAdapter>,
    cancel: CancellationToken,
    sink: WorkerEventSink,
    database: Option<Arc<Database>>,
    sequence: &mut u64,
    executor: ExecutorContext,
) {
    *sequence += 1;
    emit_state(ecosystem, "running", *sequence, &sink);
    *sequence += 1;
    emit_progress(
        task_id,
        ecosystem,
        *sequence,
        TaskStatus::Running,
        None,
        &sink,
    );
    if let Some(database) = &database {
        if !matches!(
            database.update_task(task_id, TaskStatus::Running, None),
            Ok(true)
        ) {
            *sequence += 1;
            emit_progress(
                task_id,
                ecosystem,
                *sequence,
                TaskStatus::Failed,
                Some(TaskErrorKind::Unknown),
                &sink,
            );
            *sequence += 1;
            emit_state(ecosystem, "idle", *sequence, &sink);
            return;
        }
    }

    let result = if cancel.is_cancelled() {
        Err(TaskErrorKind::CommandFailed)
    } else {
        execute_command(
            ecosystem,
            task_id,
            command,
            adapter.as_ref(),
            &executor,
            cancel.clone(),
            database.as_ref(),
            sequence,
            &sink,
        )
        .await
    };

    let (status, error) = if cancel.is_cancelled() {
        (TaskStatus::Cancelled, None)
    } else {
        match result {
            Ok(()) => (TaskStatus::Succeeded, None),
            Err(error) => (TaskStatus::Failed, Some(error)),
        }
    };
    *sequence += 1;
    emit_progress(task_id, ecosystem, *sequence, status, error, &sink);
    if let Some(database) = &database {
        if !matches!(database.update_task(task_id, status, error), Ok(true)) {
            *sequence += 1;
            emit_progress(
                task_id,
                ecosystem,
                *sequence,
                TaskStatus::Failed,
                Some(TaskErrorKind::Unknown),
                &sink,
            );
        }
    }
    *sequence += 1;
    emit_state(ecosystem, "idle", *sequence, &sink);
}

pub(crate) fn command_task(
    task_id: Uuid,
    ecosystem: Ecosystem,
    command: &WorkerCommand,
) -> PackageTask {
    let (name, operation) = match command {
        WorkerCommand::Scan(_) => ("*".to_owned(), Operation::Scan),
        WorkerCommand::RefreshDiskUsage(package) => (package.name.clone(), Operation::MeasureDisk),
        WorkerCommand::Update(task) => (task.name.clone(), Operation::Update),
        WorkerCommand::Uninstall(task) => (task.name.clone(), Operation::Uninstall),
        WorkerCommand::Shutdown => ("*".to_owned(), Operation::Scan),
    };
    PackageTask {
        task_id,
        ecosystem,
        name,
        operation,
        status: TaskStatus::Pending,
        error: None,
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_command(
    ecosystem: Ecosystem,
    task_id: Uuid,
    command: WorkerCommand,
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    sequence: &mut u64,
    sink: &WorkerEventSink,
) -> Result<(), TaskErrorKind> {
    match command {
        WorkerCommand::Scan(requested) => {
            if requested != ecosystem {
                return Err(TaskErrorKind::InvalidInput);
            }
            let packages = scan_with_retries(
                adapter,
                executor,
                ecosystem,
                cancel.clone(),
                database,
                task_id,
            )
            .await?;
            persist_and_measure_scan(
                task_id, packages, adapter, executor, cancel, database, sequence, sink,
            )
            .await
        }
        WorkerCommand::RefreshDiskUsage(package) => {
            if package.ecosystem != ecosystem {
                return Err(TaskErrorKind::InvalidInput);
            }
            refresh_package_disk_usage(
                task_id, package, adapter, executor, cancel, database, sequence, sink, true,
            )
            .await
        }
        WorkerCommand::Update(task) => {
            let affected_names =
                update_with_retries(adapter, executor, &task, cancel.clone(), database, task_id)
                    .await?;
            if let Some(database) = database {
                let mut affected_packages = Vec::new();
                for name in affected_names {
                    let Some(mut package) = database
                        .find_snapshot(ecosystem, &name)
                        .map_err(|_| TaskErrorKind::Unknown)?
                    else {
                        continue;
                    };
                    if let Some(target) = package.target_version.take() {
                        if package.resource_kind == crate::core::ResourceKind::Tap {
                            package.update_available = false;
                        } else if package.ecosystem == Ecosystem::Gem {
                            let name = package
                                .name
                                .rsplit_once('@')
                                .map_or(package.name.as_str(), |(name, _)| name)
                                .to_owned();
                            let old_id = package.id.clone();
                            package.name = format!("{name}@{target}");
                            package.id = format!(
                                "{:?}:{:?}:{}",
                                package.ecosystem, package.resource_kind, package.name
                            );
                            database
                                .delete_snapshot_and_disk_usage(ecosystem, &old_id)
                                .map_err(|_| TaskErrorKind::Unknown)?;
                            package.current_version = Some(target);
                            package.update_available = false;
                        } else {
                            package.current_version = Some(target);
                            package.update_available = false;
                        }
                    }
                    affected_packages.push(package);
                }
                let measure_tasks =
                    create_measure_tasks(task_id, &affected_packages, Some(database))?;
                let mut measurements = affected_packages.into_iter().zip(measure_tasks);
                let mut first_error = None;
                while let Some((package, measure_task)) = measurements.next() {
                    if cancel.is_cancelled() {
                        transition_measure_task(
                            &measure_task,
                            TaskStatus::Cancelled,
                            None,
                            Some(database),
                            sequence,
                            sink,
                        )?;
                        for (_, pending_task) in measurements {
                            transition_measure_task(
                                &pending_task,
                                TaskStatus::Cancelled,
                                None,
                                Some(database),
                                sequence,
                                sink,
                            )?;
                        }
                        first_error.get_or_insert(TaskErrorKind::CommandFailed);
                        break;
                    }
                    transition_measure_task(
                        &measure_task,
                        TaskStatus::Running,
                        None,
                        Some(database),
                        sequence,
                        sink,
                    )?;
                    let result = refresh_package_disk_usage(
                        measure_task.task_id,
                        package,
                        adapter,
                        executor,
                        cancel.clone(),
                        Some(database),
                        sequence,
                        sink,
                        true,
                    )
                    .await;
                    let (status, error) = match result {
                        Ok(()) => (TaskStatus::Succeeded, None),
                        Err(_) if cancel.is_cancelled() => (TaskStatus::Cancelled, None),
                        Err(error) => (TaskStatus::Failed, Some(error)),
                    };
                    transition_measure_task(
                        &measure_task,
                        status,
                        error,
                        Some(database),
                        sequence,
                        sink,
                    )?;
                    if let Err(error) = result {
                        first_error.get_or_insert(error);
                    }
                }
                if let Some(error) = first_error {
                    return Err(error);
                }
            }
            Ok(())
        }
        WorkerCommand::Uninstall(task) => {
            run_with_retries(
                adapter,
                executor,
                WorkerCommand::Uninstall(task.clone()),
                cancel,
                database,
                task_id,
            )
            .await?;
            if let Some(database) = database {
                if let Some(package) = database
                    .find_snapshot(ecosystem, &task.name)
                    .map_err(|_| TaskErrorKind::Unknown)?
                {
                    database
                        .delete_snapshot_and_disk_usage(ecosystem, &package.id)
                        .map_err(|_| TaskErrorKind::Unknown)?;
                }
            }
            Ok(())
        }
        WorkerCommand::Shutdown => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn persist_and_measure_scan(
    task_id: Uuid,
    packages: Vec<PackageRecord>,
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    sequence: &mut u64,
    sink: &WorkerEventSink,
) -> Result<(), TaskErrorKind> {
    let roots = adapter
        .resolve_install_paths(executor, cancel.clone())
        .await
        .unwrap_or_default();
    let cache = database.map_or_else(CacheStore::in_memory, |database| {
        CacheStore::from_database((*database).clone())
    });
    let service = DiskUsageService::new(cache.clone());
    let mut jobs: Vec<DiskMeasurementJob> = Vec::with_capacity(packages.len());
    let retained_ids = packages
        .iter()
        .map(|package| package.id.clone())
        .collect::<Vec<_>>();
    let measure_tasks = create_measure_tasks(task_id, &packages, database)?;
    let mut measurements = packages.into_iter().zip(measure_tasks);
    let mut preparation_error = None;
    while let Some((mut package, measure_task)) = measurements.next() {
        if cancel.is_cancelled() {
            transition_measure_task(
                &measure_task,
                TaskStatus::Cancelled,
                None,
                database,
                sequence,
                sink,
            )?;
            for job in jobs.drain(..) {
                transition_measure_task(
                    &job.task,
                    TaskStatus::Cancelled,
                    None,
                    database,
                    sequence,
                    sink,
                )?;
            }
            for (_, pending_task) in measurements {
                transition_measure_task(
                    &pending_task,
                    TaskStatus::Cancelled,
                    None,
                    database,
                    sequence,
                    sink,
                )?;
            }
            return Err(TaskErrorKind::CommandFailed);
        }
        let paths = adapter
            .resolve_package_install_paths(executor, &package, &roots, cancel.clone())
            .await
            .unwrap_or_else(|_| {
                roots.first().cloned().map_or_else(
                    || PackageInstallPaths::unavailable(std::path::PathBuf::new()),
                    PackageInstallPaths::unavailable,
                )
            });
        let cached = match cache.get_for(&package, &paths.install_root_key()) {
            Ok(cached) => cached,
            Err(_) => {
                transition_measure_task(
                    &measure_task,
                    TaskStatus::Failed,
                    Some(TaskErrorKind::Unknown),
                    database,
                    sequence,
                    sink,
                )?;
                preparation_error.get_or_insert(TaskErrorKind::Unknown);
                continue;
            }
        };
        let should_measure = match service.should_measure(&package, &paths, cached.as_ref()) {
            Ok(should_measure) => should_measure,
            Err(error) => {
                let error = map_disk_error(error);
                transition_measure_task(
                    &measure_task,
                    TaskStatus::Failed,
                    Some(error),
                    database,
                    sequence,
                    sink,
                )?;
                preparation_error.get_or_insert(error);
                continue;
            }
        };
        if !should_measure {
            transition_measure_task(
                &measure_task,
                TaskStatus::Running,
                None,
                database,
                sequence,
                sink,
            )?;
            let cached = cached.expect("cache presence checked above");
            let disk_usage = DiskUsage {
                bytes: cached.bytes,
                scanned_at: cached.scanned_at,
                status: cached.status,
            };
            package.disk_usage = Some(disk_usage);
            match save_snapshot(database, &package) {
                Ok(()) => {
                    emit_disk_usage(measure_task.task_id, sequence, sink, &package, disk_usage);
                    emit_package(measure_task.task_id, sequence, sink, package);
                    transition_measure_task(
                        &measure_task,
                        TaskStatus::Succeeded,
                        None,
                        database,
                        sequence,
                        sink,
                    )?;
                }
                Err(error) => {
                    transition_measure_task(
                        &measure_task,
                        TaskStatus::Failed,
                        Some(error),
                        database,
                        sequence,
                        sink,
                    )?;
                    preparation_error.get_or_insert(error);
                }
            }
            continue;
        }
        package.disk_usage = Some(DiskUsage {
            bytes: 0,
            scanned_at: Utc::now().timestamp(),
            status: DiskUsageStatus::Measuring,
        });
        if let Err(error) = save_snapshot(database, &package) {
            transition_measure_task(
                &measure_task,
                TaskStatus::Failed,
                Some(error),
                database,
                sequence,
                sink,
            )?;
            preparation_error.get_or_insert(error);
            continue;
        }
        emit_package(measure_task.task_id, sequence, sink, package.clone());
        jobs.push(DiskMeasurementJob {
            task: measure_task,
            package,
            paths,
            cached,
        });
    }
    let measurement_result = measure_bounded(service, jobs, cancel, database, sequence, sink).await;
    if let Some(error) = preparation_error {
        return Err(error);
    }
    measurement_result?;
    if let Some(database) = database {
        database
            .reconcile_snapshots(adapter.ecosystem(), &retained_ids)
            .map_err(|_| TaskErrorKind::Unknown)?;
    }
    Ok(())
}

async fn measure_bounded(
    service: DiskUsageService,
    jobs: Vec<DiskMeasurementJob>,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    sequence: &mut u64,
    sink: &WorkerEventSink,
) -> Result<(), TaskErrorKind> {
    let mut jobs = jobs.into_iter();
    let mut running = JoinSet::new();
    let mut first_error = None;
    for _ in 0..MAX_DISK_MEASUREMENTS {
        if let Some(job) = jobs.next() {
            transition_measure_task(
                &job.task,
                TaskStatus::Running,
                None,
                database,
                sequence,
                sink,
            )?;
            spawn_measurement(&mut running, service.clone(), job, cancel.clone());
        }
    }
    while let Some(result) = running.join_next().await {
        let (task, mut package, result) = result.map_err(|_| TaskErrorKind::Unknown)?;
        match result {
            Ok(disk_usage) => {
                package.disk_usage = Some(disk_usage);
                if let Err(error) = save_snapshot(database, &package) {
                    transition_measure_task(
                        &task,
                        TaskStatus::Failed,
                        Some(error),
                        database,
                        sequence,
                        sink,
                    )?;
                    first_error.get_or_insert(error);
                } else {
                    emit_disk_usage(task.task_id, sequence, sink, &package, disk_usage);
                    emit_package(task.task_id, sequence, sink, package);
                    transition_measure_task(
                        &task,
                        TaskStatus::Succeeded,
                        None,
                        database,
                        sequence,
                        sink,
                    )?;
                }
            }
            Err(error) => {
                let error = map_disk_error(error);
                let status = if cancel.is_cancelled() {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                };
                let persisted_error = (!matches!(status, TaskStatus::Cancelled)).then_some(error);
                transition_measure_task(&task, status, persisted_error, database, sequence, sink)?;
                first_error.get_or_insert(error);
            }
        }
        if let Some(job) = jobs.next() {
            transition_measure_task(
                &job.task,
                TaskStatus::Running,
                None,
                database,
                sequence,
                sink,
            )?;
            spawn_measurement(&mut running, service.clone(), job, cancel.clone());
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn spawn_measurement(
    running: &mut JoinSet<(
        PackageTask,
        PackageRecord,
        Result<DiskUsage, DiskUsageError>,
    )>,
    service: DiskUsageService,
    job: DiskMeasurementJob,
    cancel: CancellationToken,
) {
    running.spawn_blocking(move || {
        let DiskMeasurementJob {
            task,
            package,
            paths,
            cached,
        } = job;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.measure_if_needed(&package, &paths, cached, &cancel)
        }))
        .unwrap_or_else(|_| {
            Err(DiskUsageError::Io(std::io::Error::other(
                "disk measurement worker panicked",
            )))
        });
        (task, package, result)
    });
}

#[allow(clippy::too_many_arguments)]
async fn refresh_package_disk_usage(
    task_id: Uuid,
    mut package: PackageRecord,
    adapter: &dyn EcosystemAdapter,
    executor: &ExecutorContext,
    cancel: CancellationToken,
    database: Option<&Arc<Database>>,
    sequence: &mut u64,
    sink: &WorkerEventSink,
    force: bool,
) -> Result<(), TaskErrorKind> {
    let roots = adapter
        .resolve_install_paths(executor, cancel.clone())
        .await
        .unwrap_or_default();
    let paths = adapter
        .resolve_package_install_paths(executor, &package, &roots, cancel.clone())
        .await
        .unwrap_or_else(|_| {
            roots.first().cloned().map_or_else(
                || PackageInstallPaths::unavailable(std::path::PathBuf::new()),
                PackageInstallPaths::unavailable,
            )
        });
    let cache = database.map_or_else(CacheStore::in_memory, |database| {
        CacheStore::from_database((*database).clone())
    });
    if force {
        cache
            .invalidate_after_success(&package)
            .map_err(|_| TaskErrorKind::Unknown)?;
    }
    let service = DiskUsageService::new(cache.clone());
    let cached = cache
        .get_for(&package, &paths.install_root_key())
        .map_err(|_| TaskErrorKind::Unknown)?;
    let measured = tokio::task::spawn_blocking({
        let cancel = cancel.clone();
        let package = package.clone();
        move || service.measure_if_needed(&package, &paths, cached, &cancel)
    })
    .await
    .map_err(|_| TaskErrorKind::Unknown)?
    .map_err(map_disk_error)?;
    package.disk_usage = Some(measured);
    save_snapshot(database, &package)?;
    emit_disk_usage(task_id, sequence, sink, &package, measured);
    emit_package(task_id, sequence, sink, package);
    Ok(())
}

fn create_measure_tasks(
    parent_task_id: Uuid,
    packages: &[PackageRecord],
    database: Option<&Arc<Database>>,
) -> Result<Vec<PackageTask>, TaskErrorKind> {
    let tasks = packages
        .iter()
        .map(|package| {
            PackageTask::new(
                package.ecosystem,
                package.name.clone(),
                Operation::MeasureDisk,
            )
        })
        .collect::<Vec<_>>();
    if let Some(database) = database {
        let batch_id = database
            .append_tasks_to_task_batch(parent_task_id, &tasks)
            .map_err(|_| TaskErrorKind::Unknown)?;
        if batch_id.is_none() {
            return Err(TaskErrorKind::Unknown);
        }
    }
    Ok(tasks)
}

fn transition_measure_task(
    task: &PackageTask,
    status: TaskStatus,
    error: Option<TaskErrorKind>,
    database: Option<&Arc<Database>>,
    sequence: &mut u64,
    sink: &WorkerEventSink,
) -> Result<(), TaskErrorKind> {
    if let Some(database) = database {
        if !matches!(database.update_task(task.task_id, status, error), Ok(true)) {
            return Err(TaskErrorKind::Unknown);
        }
    }
    *sequence += 1;
    emit_progress(task.task_id, task.ecosystem, *sequence, status, error, sink);
    Ok(())
}

fn save_snapshot(
    database: Option<&Arc<Database>>,
    package: &PackageRecord,
) -> Result<(), TaskErrorKind> {
    if let Some(database) = database {
        database
            .save_snapshot(package)
            .map_err(|_| TaskErrorKind::Unknown)?;
    }
    Ok(())
}

fn map_disk_error(error: DiskUsageError) -> TaskErrorKind {
    match error {
        DiskUsageError::Cancelled => TaskErrorKind::CommandFailed,
        DiskUsageError::Io(_) | DiskUsageError::Persistence(_) => TaskErrorKind::Unknown,
    }
}

fn emit_package(task_id: Uuid, sequence: &mut u64, sink: &WorkerEventSink, package: PackageRecord) {
    *sequence += 1;
    sink(WorkerEvent::PackageChanged {
        task_id,
        ecosystem: package.ecosystem,
        sequence: *sequence,
        package,
        emitted_at: Utc::now().timestamp(),
    });
}

fn emit_disk_usage(
    task_id: Uuid,
    sequence: &mut u64,
    sink: &WorkerEventSink,
    package: &PackageRecord,
    disk_usage: DiskUsage,
) {
    *sequence += 1;
    sink(WorkerEvent::DiskUsage {
        task_id,
        ecosystem: package.ecosystem,
        sequence: *sequence,
        package_id: package.id.clone(),
        disk_usage,
        emitted_at: Utc::now().timestamp(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_adapter_is_explicitly_unavailable() {
        let result = NoopAdapter
            .run(
                WorkerCommand::Scan(Ecosystem::Homebrew),
                CancellationToken::new(),
            )
            .await;
        assert!(matches!(result, Err(TaskErrorKind::Unknown)));
    }
}

fn emit_state(ecosystem: Ecosystem, state: &str, sequence: u64, sink: &WorkerEventSink) {
    sink(WorkerEvent::WorkerState {
        ecosystem,
        sequence,
        state: state.to_owned(),
        emitted_at: Utc::now().timestamp(),
    });
}

fn emit_progress(
    task_id: Uuid,
    ecosystem: Ecosystem,
    sequence: u64,
    status: TaskStatus,
    error: Option<TaskErrorKind>,
    sink: &WorkerEventSink,
) {
    sink(WorkerEvent::TaskProgress {
        task_id,
        ecosystem,
        sequence,
        status,
        completed: u64::from(matches!(
            status,
            TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled
        )),
        total: 1,
        message: None,
        error,
        emitted_at: Utc::now().timestamp(),
    });
}
