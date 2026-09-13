use std::fs;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use updaddy_lib::adapters::{
    package_record, CommandRunner, EcosystemAdapter, ExecutorContext, HomebrewAdapter,
};
use updaddy_lib::core::{
    Ecosystem, Operation, PackageRecord, PackageTask, ResourceKind, TaskErrorKind, TaskStatus,
};
use updaddy_lib::disk_usage::{resolve_package_paths, CacheStore, DiskUsageService};
use updaddy_lib::executor::{CommandResult, CommandSpec, ProcessError};
use updaddy_lib::persistence::Database;
use updaddy_lib::workers::{
    SupervisorContext, WorkerCommand, WorkerEvent, WorkerEventSink, WorkerSupervisor,
};

fn package(name: &str) -> updaddy_lib::core::PackageRecord {
    package_record(
        Ecosystem::Npm,
        ResourceKind::Package,
        name,
        Some("1.0.0".into()),
        None,
    )
}

fn updateable_package(name: &str) -> PackageRecord {
    package_record(
        Ecosystem::Npm,
        ResourceKind::Package,
        name,
        Some("1.0.0".into()),
        Some("2.0.0".into()),
    )
}

fn cache_entry(
    package: &PackageRecord,
    install_root: &std::path::Path,
    bytes: u64,
) -> updaddy_lib::persistence::DiskUsageCacheEntry {
    updaddy_lib::persistence::DiskUsageCacheEntry {
        ecosystem: match package.ecosystem {
            Ecosystem::Homebrew => "homebrew",
            Ecosystem::Npm => "npm",
            Ecosystem::Pip => "pip",
            Ecosystem::Gem => "gem",
            Ecosystem::Rustup => "rustup",
        }
        .into(),
        package_id: package.id.clone(),
        installed_version: package.current_version.clone().unwrap_or_default(),
        install_root: install_root.to_string_lossy().into_owned(),
        path_signature: "stale".into(),
        bytes,
        status: updaddy_lib::core::DiskUsageStatus::Ready,
        scanned_at: 10,
    }
}

fn tap_package(name: &str) -> PackageRecord {
    package_record(
        Ecosystem::Homebrew,
        ResourceKind::Tap,
        format!("tap:{name}"),
        None,
        Some("updated".into()),
    )
}

#[test]
fn cache_miss_measures_only_the_requested_package() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("a")).unwrap();
    fs::create_dir(directory.path().join("b")).unwrap();
    fs::write(directory.path().join("a/data"), vec![0; 4096]).unwrap();
    fs::write(directory.path().join("b/data"), vec![0; 8192]).unwrap();
    let package = package("a");
    let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);
    let cache = CacheStore::in_memory();
    let service = DiskUsageService::new(cache.clone());

    let usage = service.measure(&package, &paths).unwrap();

    assert_eq!(usage.bytes, 4096);
    assert_eq!(cache.get(&package).unwrap().unwrap().bytes, 4096);
}

#[test]
fn uninstall_cleanup_removes_only_one_package_snapshot_and_cache() {
    let directory = tempfile::tempdir().unwrap();
    let database =
        std::sync::Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let a = package("a");
    let b = package("b");
    database.save_snapshot(&a).unwrap();
    database.save_snapshot(&b).unwrap();
    let cache = CacheStore::from_database(database.clone());
    for (package, bytes) in [(&a, 100), (&b, 200)] {
        cache
            .put(cache_entry(package, std::path::Path::new("/global"), bytes))
            .unwrap();
    }

    database
        .delete_snapshot_and_disk_usage(Ecosystem::Npm, &a.id)
        .unwrap();

    assert!(cache.get(&a).unwrap().is_none());
    assert_eq!(cache.get(&b).unwrap().unwrap().bytes, 200);
    assert!(database.load_snapshot(&a.id).unwrap().is_none());
    assert_eq!(database.load_snapshot(&b.id).unwrap(), Some(b));
}

struct ScanAdapter {
    root: std::path::PathBuf,
}

#[async_trait]
impl EcosystemAdapter for ScanAdapter {
    fn ecosystem(&self) -> Ecosystem {
        Ecosystem::Npm
    }

    async fn scan(&self, _context: &ExecutorContext) -> Result<Vec<PackageRecord>, TaskErrorKind> {
        Ok(vec![package("a"), package("b")])
    }

    async fn resolve_install_paths(
        &self,
        _context: &ExecutorContext,
        _cancel: CancellationToken,
    ) -> Result<Vec<std::path::PathBuf>, TaskErrorKind> {
        Ok(vec![self.root.clone()])
    }

    async fn execute(
        &self,
        _context: &ExecutorContext,
        _task: &PackageTask,
        _cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        Ok(())
    }

    async fn uninstall(
        &self,
        _context: &ExecutorContext,
        _task: &PackageTask,
        _cancel: CancellationToken,
    ) -> Result<(), TaskErrorKind> {
        Ok(())
    }
}

type Events = Arc<(Mutex<Vec<WorkerEvent>>, Condvar)>;

fn start_supervisor(
    database: Arc<Database>,
    root: std::path::PathBuf,
) -> (WorkerSupervisor, Events) {
    let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let sink: WorkerEventSink = {
        let events = events.clone();
        Arc::new(move |event| {
            events.0.lock().unwrap().push(event);
            events.1.notify_all();
        })
    };
    let mut context = SupervisorContext::new(sink);
    context.database = Some(database);
    context
        .adapters
        .insert(Ecosystem::Npm, Arc::new(ScanAdapter { root }));
    (WorkerSupervisor::start(context), events)
}

fn wait_for_success(events: &Events, task_id: uuid::Uuid) -> Vec<WorkerEvent> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut guard = events.0.lock().unwrap();
    loop {
        if let Some(status) = guard.iter().find_map(|event| match event {
            WorkerEvent::TaskProgress {
                task_id: id,
                status,
                ..
            } if *id == task_id
                && matches!(
                    status,
                    TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled
                ) =>
            {
                Some(*status)
            }
            _ => None,
        }) {
            assert_eq!(status, TaskStatus::Succeeded);
            return guard.clone();
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "等待任务完成超时");
        let (next, result) = events.1.wait_timeout(guard, remaining).unwrap();
        guard = next;
        assert!(!result.timed_out(), "等待任务完成超时");
    }
}

#[test]
fn scan_persists_snapshot_and_emits_disk_usage() {
    let directory = tempfile::tempdir().unwrap();
    let install_root = directory.path().join("node_modules");
    fs::create_dir_all(install_root.join("a")).unwrap();
    fs::create_dir_all(install_root.join("b")).unwrap();
    fs::write(install_root.join("a/data"), vec![0; 128]).unwrap();
    fs::write(install_root.join("b/data"), vec![0; 64]).unwrap();
    let database = Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let (supervisor, events) = start_supervisor(database.clone(), install_root);

    let task_id = supervisor
        .submit(WorkerCommand::Scan(Ecosystem::Npm))
        .unwrap();
    let events = wait_for_success(&events, task_id);
    let disk_task_ids = events
        .iter()
        .filter_map(|event| match event {
            WorkerEvent::DiskUsage {
                task_id: id,
                disk_usage,
                ..
            } if disk_usage.bytes == 128 || disk_usage.bytes == 64 => Some(*id),
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(disk_task_ids.len(), 2);
    assert!(!disk_task_ids.contains(&task_id));

    let batch_id = database.batch_id_for_task(task_id).unwrap().unwrap();
    let batch = database.load_batch(batch_id).unwrap().unwrap();
    let measure_tasks = batch
        .tasks
        .iter()
        .filter(|task| task.operation == Operation::MeasureDisk)
        .collect::<Vec<_>>();
    assert_eq!(measure_tasks.len(), 2);
    assert!(measure_tasks
        .iter()
        .all(|task| task.status == TaskStatus::Succeeded));
    assert_eq!(
        measure_tasks
            .iter()
            .map(|task| task.task_id)
            .collect::<std::collections::HashSet<_>>(),
        disk_task_ids
    );

    let snapshot = database.load_snapshot(&package("a").id).unwrap().unwrap();
    assert_eq!(snapshot.disk_usage.unwrap().bytes, 128);
}

#[test]
fn cache_hit_still_uses_a_succeeded_measurement_task_per_package() {
    let directory = tempfile::tempdir().unwrap();
    let install_root = directory.path().join("node_modules");
    fs::create_dir_all(install_root.join("a")).unwrap();
    fs::create_dir_all(install_root.join("b")).unwrap();
    fs::write(install_root.join("a/data"), vec![0; 128]).unwrap();
    fs::write(install_root.join("b/data"), vec![0; 64]).unwrap();
    let database = Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let (supervisor, events) = start_supervisor(database.clone(), install_root);

    let first_scan = supervisor
        .submit(WorkerCommand::Scan(Ecosystem::Npm))
        .unwrap();
    wait_for_success(&events, first_scan);
    let second_scan = supervisor
        .submit(WorkerCommand::Scan(Ecosystem::Npm))
        .unwrap();
    let emitted = wait_for_success(&events, second_scan);
    let batch_id = database.batch_id_for_task(second_scan).unwrap().unwrap();
    let batch = database.load_batch(batch_id).unwrap().unwrap();
    let measure_tasks = batch
        .tasks
        .iter()
        .filter(|task| task.operation == Operation::MeasureDisk)
        .collect::<Vec<_>>();
    let measure_task_ids = measure_tasks
        .iter()
        .map(|task| task.task_id)
        .collect::<std::collections::HashSet<_>>();
    let emitted_task_ids = emitted
        .iter()
        .filter_map(|event| match event {
            WorkerEvent::DiskUsage { task_id, .. } if measure_task_ids.contains(task_id) => {
                Some(*task_id)
            }
            _ => None,
        })
        .collect::<std::collections::HashSet<_>>();

    assert_eq!(measure_tasks.len(), 2);
    assert!(measure_tasks
        .iter()
        .all(|task| task.status == TaskStatus::Succeeded));
    assert_eq!(emitted_task_ids, measure_task_ids);
}

#[test]
fn manual_refresh_remeasures_only_the_requested_package() {
    let directory = tempfile::tempdir().unwrap();
    let install_root = directory.path().join("node_modules");
    fs::create_dir_all(install_root.join("a")).unwrap();
    fs::create_dir_all(install_root.join("b")).unwrap();
    fs::write(install_root.join("a/data"), vec![0; 128]).unwrap();
    fs::write(install_root.join("b/data"), vec![0; 512]).unwrap();
    let database = Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let a = package("a");
    let b = package("b");
    database.save_snapshot(&a).unwrap();
    database.save_snapshot(&b).unwrap();
    let cache = CacheStore::from_database(database.clone());
    cache.put(cache_entry(&a, &install_root, 100)).unwrap();
    cache.put(cache_entry(&b, &install_root, 200)).unwrap();
    let (supervisor, events) = start_supervisor(database.clone(), install_root.clone());

    let task_id = supervisor
        .submit(WorkerCommand::RefreshDiskUsage(a.clone()))
        .unwrap();
    wait_for_success(&events, task_id);

    assert_eq!(cache.get(&a).unwrap().unwrap().bytes, 128);
    assert_eq!(cache.get(&b).unwrap().unwrap().bytes, 200);
    assert_eq!(
        database.load_snapshot(&b.id).unwrap().unwrap().disk_usage,
        None
    );
}

#[test]
fn successful_update_replaces_only_the_updated_package_cache() {
    let directory = tempfile::tempdir().unwrap();
    let install_root = directory.path().join("node_modules");
    fs::create_dir_all(install_root.join("a")).unwrap();
    fs::write(install_root.join("a/data"), vec![0; 256]).unwrap();
    let database = Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let a = updateable_package("a");
    let b = package("b");
    database.save_snapshot(&a).unwrap();
    database.save_snapshot(&b).unwrap();
    let cache = CacheStore::from_database(database.clone());
    cache.put(cache_entry(&a, &install_root, 100)).unwrap();
    cache.put(cache_entry(&b, &install_root, 200)).unwrap();
    let (supervisor, events) = start_supervisor(database.clone(), install_root.clone());

    let task_id = supervisor
        .submit(WorkerCommand::Update(PackageTask::new(
            Ecosystem::Npm,
            "a",
            Operation::Update,
        )))
        .unwrap();
    wait_for_success(&events, task_id);

    let updated = database.load_snapshot(&a.id).unwrap().unwrap();
    assert_eq!(updated.current_version.as_deref(), Some("2.0.0"));
    assert_eq!(updated.disk_usage.unwrap().bytes, 256);
    assert!(database
        .find_disk_usage("npm", &a.id, "1.0.0", &install_root.to_string_lossy())
        .unwrap()
        .is_none());
    assert_eq!(cache.get(&b).unwrap().unwrap().bytes, 200);
}

#[test]
fn successful_uninstall_removes_only_the_requested_package() {
    let directory = tempfile::tempdir().unwrap();
    let install_root = directory.path().join("node_modules");
    let database = Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let a = package("a");
    let b = package("b");
    database.save_snapshot(&a).unwrap();
    database.save_snapshot(&b).unwrap();
    let cache = CacheStore::from_database(database.clone());
    cache.put(cache_entry(&a, &install_root, 100)).unwrap();
    cache.put(cache_entry(&b, &install_root, 200)).unwrap();
    let (supervisor, events) = start_supervisor(database.clone(), install_root);

    let task_id = supervisor
        .submit(WorkerCommand::Uninstall(PackageTask::new(
            Ecosystem::Npm,
            "a",
            Operation::Uninstall,
        )))
        .unwrap();
    wait_for_success(&events, task_id);

    assert!(events.0.lock().unwrap().iter().any(|event| matches!(
        event,
        WorkerEvent::PackageRemoved { package_id, .. } if package_id == &a.id
    )));

    assert!(database.load_snapshot(&a.id).unwrap().is_none());
    assert!(cache.get(&a).unwrap().is_none());
    assert_eq!(database.load_snapshot(&b.id).unwrap(), Some(b.clone()));
    assert_eq!(cache.get(&b).unwrap().unwrap().bytes, 200);
}

struct BrewUpdateRunner;

#[async_trait]
impl CommandRunner for BrewUpdateRunner {
    async fn run(
        &self,
        spec: CommandSpec,
        _cancel: CancellationToken,
    ) -> Result<CommandResult, ProcessError> {
        assert_eq!(spec.program, "brew");
        let args = spec.args.iter().map(String::as_str).collect::<Vec<_>>();
        let stdout = match args.as_slice() {
            ["--prefix"] => "/opt/homebrew\n",
            ["update"] => "Updated 1 tap (acme/one).\n",
            _ => panic!("unexpected brew command: {args:?}"),
        };
        Ok(CommandResult {
            status: std::process::Command::new("sh")
                .args(["-c", "exit 0"])
                .status()
                .unwrap(),
            stdout: stdout.into(),
            stderr: String::new(),
            duration: Duration::ZERO,
        })
    }
}

#[test]
fn homebrew_update_invalidates_only_taps_changed_by_the_command() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(Database::open(directory.path().join("updaddy.sqlite")).unwrap());
    let changed = tap_package("acme/one");
    let unchanged = tap_package("acme/two");
    database.save_snapshot(&changed).unwrap();
    database.save_snapshot(&unchanged).unwrap();
    let tap_root = std::path::Path::new("/opt/homebrew/Library/Taps");
    let cache = CacheStore::from_database(database.clone());
    cache.put(cache_entry(&changed, tap_root, 100)).unwrap();
    cache.put(cache_entry(&unchanged, tap_root, 200)).unwrap();
    let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let sink: WorkerEventSink = {
        let events = events.clone();
        Arc::new(move |event| {
            events.0.lock().unwrap().push(event);
            events.1.notify_all();
        })
    };
    let mut context = SupervisorContext::new(sink);
    context.database = Some(database.clone());
    context
        .adapters
        .insert(Ecosystem::Homebrew, Arc::new(HomebrewAdapter::new()));
    context.executor = ExecutorContext::with_runner(Arc::new(BrewUpdateRunner));
    let supervisor = WorkerSupervisor::start(context);

    let task_id = supervisor
        .submit(WorkerCommand::Update(PackageTask::new(
            Ecosystem::Homebrew,
            changed.name.clone(),
            Operation::Update,
        )))
        .unwrap();
    let emitted = wait_for_success(&events, task_id);

    let changed_cache = cache.get(&changed).unwrap().unwrap();
    assert_eq!(changed_cache.bytes, 0);
    assert_eq!(
        changed_cache.status,
        updaddy_lib::core::DiskUsageStatus::Unavailable
    );
    assert_eq!(cache.get(&unchanged).unwrap().unwrap().bytes, 200);
    assert!(emitted.iter().any(|event| {
        matches!(event, WorkerEvent::DiskUsage { task_id: disk_task_id, package_id, .. }
            if *disk_task_id != task_id && package_id == &changed.id)
    }));
    assert!(!emitted.iter().any(|event| {
        matches!(event, WorkerEvent::DiskUsage { package_id, .. } if package_id == &unchanged.id)
    }));
}
