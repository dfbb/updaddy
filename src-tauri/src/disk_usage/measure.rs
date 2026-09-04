use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use walkdir::WalkDir;

use crate::core::{DiskUsage, DiskUsageStatus, Ecosystem, PackageRecord, ResourceKind};
use crate::persistence::DiskUsageCacheEntry;

use super::cache::{ecosystem_name, installed_version};
use super::CacheStore;

#[derive(Debug, Error)]
pub enum DiskUsageError {
    #[error("disk measurement was cancelled")]
    Cancelled,
    #[error("disk measurement failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("disk cache failed: {0}")]
    Persistence(crate::persistence::PersistenceError),
}

pub type Result<T> = std::result::Result<T, DiskUsageError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInstallPaths {
    pub install_root: PathBuf,
    pub paths: Vec<PathBuf>,
    pub measurable: bool,
}

impl PackageInstallPaths {
    pub fn unavailable(install_root: impl Into<PathBuf>) -> Self {
        Self {
            install_root: install_root.into(),
            paths: Vec::new(),
            measurable: false,
        }
    }

    fn from_candidates(candidates: Vec<(PathBuf, Vec<PathBuf>)>) -> Self {
        if let Some((root, paths)) = candidates
            .iter()
            .find(|(_, paths)| paths.iter().any(|path| path.exists()))
        {
            return Self {
                install_root: root.clone(),
                paths: paths.iter().filter(|path| path.exists()).cloned().collect(),
                measurable: true,
            };
        }
        candidates.into_iter().next().map_or_else(
            || Self::unavailable(PathBuf::new()),
            |(root, _)| Self::unavailable(root),
        )
    }

    pub fn install_root_key(&self) -> String {
        self.install_root.to_string_lossy().into_owned()
    }
}

#[derive(Clone)]
pub struct DiskUsageService {
    cache: CacheStore,
}

impl DiskUsageService {
    pub fn new(cache: CacheStore) -> Self {
        Self { cache }
    }

    pub fn measure(
        &self,
        package: &PackageRecord,
        install_paths: &PackageInstallPaths,
    ) -> Result<DiskUsage> {
        self.measure_with_cancel(package, install_paths, &CancellationToken::new())
    }

    pub fn measure_with_cancel(
        &self,
        package: &PackageRecord,
        install_paths: &PackageInstallPaths,
        cancel: &CancellationToken,
    ) -> Result<DiskUsage> {
        if cancel.is_cancelled() {
            return Err(DiskUsageError::Cancelled);
        }
        let signature =
            path_signature(&install_paths.paths).unwrap_or_else(|_| "unavailable".to_owned());
        let disk_usage = if install_paths.measurable {
            match measure_paths_with_cancel(&install_paths.paths, cancel) {
                Ok(mut usage) => {
                    usage.scanned_at = Utc::now().timestamp();
                    usage
                }
                Err(DiskUsageError::Cancelled) => return Err(DiskUsageError::Cancelled),
                Err(_) => DiskUsage {
                    bytes: 0,
                    scanned_at: Utc::now().timestamp(),
                    status: DiskUsageStatus::Unavailable,
                },
            }
        } else {
            DiskUsage {
                bytes: 0,
                scanned_at: Utc::now().timestamp(),
                status: DiskUsageStatus::Unavailable,
            }
        };
        self.cache
            .put(cache_entry(package, install_paths, signature, disk_usage))?;
        Ok(disk_usage)
    }

    pub fn should_measure(
        &self,
        package: &PackageRecord,
        install_paths: &PackageInstallPaths,
        cached: Option<&DiskUsageCacheEntry>,
    ) -> Result<bool> {
        let Ok(signature) = path_signature(&install_paths.paths) else {
            return Ok(true);
        };
        Ok(cached.map_or(true, |entry| {
            entry.ecosystem != ecosystem_name(package.ecosystem)
                || entry.package_id != package.id
                || entry.installed_version != installed_version(package)
                || entry.install_root != install_paths.install_root_key()
                || entry.path_signature != signature
        }))
    }

    pub fn measure_if_needed(
        &self,
        package: &PackageRecord,
        install_paths: &PackageInstallPaths,
        cached: Option<DiskUsageCacheEntry>,
        cancel: &CancellationToken,
    ) -> Result<DiskUsage> {
        if !self.should_measure(package, install_paths, cached.as_ref())? {
            let entry = cached.expect("cache presence checked above");
            return Ok(DiskUsage {
                bytes: entry.bytes,
                scanned_at: entry.scanned_at,
                status: entry.status,
            });
        }
        self.measure_with_cancel(package, install_paths, cancel)
    }
}

fn cache_entry(
    package: &PackageRecord,
    paths: &PackageInstallPaths,
    signature: String,
    disk_usage: DiskUsage,
) -> DiskUsageCacheEntry {
    DiskUsageCacheEntry {
        ecosystem: ecosystem_name(package.ecosystem).into(),
        package_id: package.id.clone(),
        installed_version: installed_version(package).into(),
        install_root: paths.install_root_key(),
        path_signature: signature,
        bytes: disk_usage.bytes,
        status: disk_usage.status,
        scanned_at: disk_usage.scanned_at,
    }
}

pub fn measure_paths(paths: &[PathBuf]) -> Result<DiskUsage> {
    measure_paths_with_cancel(paths, &CancellationToken::new())
}

fn measure_paths_with_cancel(paths: &[PathBuf], cancel: &CancellationToken) -> Result<DiskUsage> {
    let mut bytes = 0_u64;
    let mut seen_files = HashSet::new();
    for root in paths {
        for entry in WalkDir::new(root).follow_links(false) {
            if cancel.is_cancelled() {
                return Err(DiskUsageError::Cancelled);
            }
            let entry = entry.map_err(|error| {
                error
                    .into_io_error()
                    .unwrap_or_else(|| std::io::Error::other("directory walk failed"))
            })?;
            if entry.file_type().is_symlink() || !entry.file_type().is_file() {
                continue;
            }
            let metadata = entry.metadata().map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::Other, error.to_string())
            })?;
            let identity = file_identity(entry.path(), &metadata);
            if seen_files.insert(identity) {
                bytes = bytes.checked_add(metadata.len()).ok_or_else(|| {
                    std::io::Error::other("disk usage exceeds the supported range")
                })?;
            }
        }
    }
    Ok(DiskUsage {
        bytes,
        scanned_at: Utc::now().timestamp(),
        status: DiskUsageStatus::Ready,
    })
}

#[cfg(unix)]
fn file_identity(_path: &Path, metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn file_identity(path: &Path, _metadata: &fs::Metadata) -> (u64, u64) {
    (
        stable_hash(path.as_os_str().to_string_lossy().as_bytes()),
        0,
    )
}

pub fn path_signature(paths: &[PathBuf]) -> Result<String> {
    let anchor = paths.first().cloned();
    let mut sorted = paths.to_vec();
    sorted.sort();
    let mut hash = 0xcbf29ce484222325_u64;
    for path in sorted {
        hash_bytes(&mut hash, path.as_os_str().to_string_lossy().as_bytes());
    }
    if let Some(anchor) = anchor {
        // The path list captures changes in package metadata such as pip's RECORD. Only the
        // package-level anchor is statted so a cache hit does not repeat the full size scan.
        match fs::symlink_metadata(&anchor) {
            Ok(metadata) => {
                hash_bytes(&mut hash, &[u8::from(metadata.file_type().is_symlink())]);
                hash_bytes(&mut hash, &metadata.len().to_le_bytes());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    hash_bytes(&mut hash, &metadata.dev().to_le_bytes());
                    hash_bytes(&mut hash, &metadata.ino().to_le_bytes());
                    hash_bytes(&mut hash, &metadata.mtime().to_le_bytes());
                    hash_bytes(&mut hash, &metadata.mtime_nsec().to_le_bytes());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                hash_bytes(&mut hash, b"missing");
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(format!("{hash:016x}"))
}

#[cfg(not(unix))]
fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    hash_bytes(&mut hash, bytes);
    hash
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

pub fn resolve_package_paths(package: &PackageRecord, roots: &[PathBuf]) -> PackageInstallPaths {
    match (package.ecosystem, package.resource_kind) {
        (Ecosystem::Homebrew, ResourceKind::Formula) => {
            resolve_named_root(roots, "Cellar", package.name.as_str())
        }
        (Ecosystem::Homebrew, ResourceKind::Cask) => resolve_named_root(
            roots,
            "Caskroom",
            package.name.strip_prefix("cask:").unwrap_or(&package.name),
        ),
        (Ecosystem::Homebrew, ResourceKind::Tap) => resolve_tap(
            roots,
            package.name.strip_prefix("tap:").unwrap_or(&package.name),
        ),
        (Ecosystem::Npm, ResourceKind::Package) => resolve_npm(roots, &package.name),
        (Ecosystem::Pip, ResourceKind::Package) => resolve_pip(roots, &package.name),
        (Ecosystem::Gem, ResourceKind::Package) => resolve_gem(package, roots),
        (Ecosystem::Rustup, ResourceKind::Toolchain) => resolve_named_root(
            roots,
            "toolchains",
            package
                .name
                .strip_prefix("toolchain:")
                .unwrap_or(&package.name),
        ),
        (Ecosystem::Rustup, ResourceKind::Component | ResourceKind::Target) => {
            roots.first().cloned().map_or_else(
                || PackageInstallPaths::unavailable(PathBuf::new()),
                PackageInstallPaths::unavailable,
            )
        }
        _ => PackageInstallPaths::unavailable(PathBuf::new()),
    }
}

fn resolve_named_root(roots: &[PathBuf], expected_root: &str, name: &str) -> PackageInstallPaths {
    let Some(relative) = safe_relative(name) else {
        return PackageInstallPaths::unavailable(PathBuf::new());
    };
    PackageInstallPaths::from_candidates(
        roots
            .iter()
            .filter(|root| root.file_name().is_some_and(|part| part == expected_root))
            .map(|root| (root.clone(), vec![root.join(&relative)]))
            .collect(),
    )
}

fn resolve_npm(roots: &[PathBuf], name: &str) -> PackageInstallPaths {
    let Some(relative) = safe_relative(name) else {
        return PackageInstallPaths::unavailable(PathBuf::new());
    };
    PackageInstallPaths::from_candidates(
        roots
            .iter()
            .map(|root| (root.clone(), vec![root.join(&relative)]))
            .collect(),
    )
}

fn resolve_tap(roots: &[PathBuf], name: &str) -> PackageInstallPaths {
    let Some((owner, repository)) = name.split_once('/') else {
        return PackageInstallPaths::unavailable(PathBuf::new());
    };
    if safe_relative(owner).is_none() || safe_relative(repository).is_none() {
        return PackageInstallPaths::unavailable(PathBuf::new());
    }
    PackageInstallPaths::from_candidates(
        roots
            .iter()
            .filter(|root| root.file_name().is_some_and(|part| part == "Taps"))
            .map(|root| {
                let owner_root = root.join(owner);
                (
                    root.clone(),
                    vec![
                        owner_root.join(format!("homebrew-{repository}")),
                        owner_root.join(repository),
                    ],
                )
            })
            .collect(),
    )
}

fn resolve_gem(package: &PackageRecord, roots: &[PathBuf]) -> PackageInstallPaths {
    let (name, version) = package.name.rsplit_once('@').map_or(
        (package.name.as_str(), installed_version(package)),
        |parts| parts,
    );
    if safe_relative(name).is_none() || safe_relative(version).is_none() {
        return PackageInstallPaths::unavailable(PathBuf::new());
    }
    let directory = format!("{name}-{version}");
    PackageInstallPaths::from_candidates(
        roots
            .iter()
            .filter(|root| {
                !matches!(
                    root.file_name().and_then(|part| part.to_str()),
                    Some("specifications")
                ) && root
                    .extension()
                    .map_or(true, |extension| extension != "gemspec")
            })
            .map(|root| {
                (
                    root.clone(),
                    vec![
                        root.join("gems").join(&directory),
                        root.join("specifications")
                            .join(format!("{directory}.gemspec")),
                    ],
                )
            })
            .collect(),
    )
}

fn resolve_pip(roots: &[PathBuf], name: &str) -> PackageInstallPaths {
    let normalized_name = normalize_python_name(name);
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if !path.is_dir() || !is_python_metadata_dir(&path) {
                continue;
            }
            if metadata_distribution_name(&path)
                .is_some_and(|found| normalize_python_name(&found) == normalized_name)
            {
                let record = path.join("RECORD");
                let Ok(contents) = fs::read_to_string(record) else {
                    return PackageInstallPaths::unavailable(root.clone());
                };
                let Some(record_paths) = complete_record_paths(root, &contents) else {
                    return PackageInstallPaths::unavailable(root.clone());
                };
                let mut paths = vec![path.clone()];
                paths.extend(record_paths);
                return PackageInstallPaths {
                    install_root: root.clone(),
                    paths,
                    measurable: true,
                };
            }
        }
    }
    roots.first().cloned().map_or_else(
        || PackageInstallPaths::unavailable(PathBuf::new()),
        PackageInstallPaths::unavailable,
    )
}

fn safe_relative(value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

fn is_python_metadata_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".dist-info"))
}

fn complete_record_paths(root: &Path, contents: &str) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in contents.lines() {
        let relative = parse_record_path(line.strip_suffix('\r').unwrap_or(line))?;
        if relative.as_os_str().is_empty() || relative.is_absolute() {
            return None;
        }
        let candidate = root.join(relative);
        let metadata = fs::symlink_metadata(&candidate).ok()?;
        if !metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
            return None;
        }
        paths.push(candidate);
    }
    (!paths.is_empty()).then_some(paths)
}

fn metadata_distribution_name(path: &Path) -> Option<String> {
    ["METADATA", "PKG-INFO"].into_iter().find_map(|file| {
        fs::read_to_string(path.join(file))
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    line.strip_prefix("Name:")
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                })
            })
    })
}

fn normalize_python_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            if !separator {
                normalized.push('-');
                separator = true;
            }
        } else {
            normalized.extend(character.to_lowercase());
            separator = false;
        }
    }
    normalized
}

fn parse_record_path(line: &str) -> Option<PathBuf> {
    #[derive(Clone, Copy)]
    enum State {
        Start,
        Unquoted,
        Quoted,
        AfterQuote,
    }

    let mut fields = Vec::new();
    let mut field = String::new();
    let mut state = State::Start;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match state {
            State::Start => match character {
                ',' => fields.push(String::new()),
                '"' => state = State::Quoted,
                '\r' | '\n' => return None,
                _ => {
                    field.push(character);
                    state = State::Unquoted;
                }
            },
            State::Unquoted => match character {
                ',' => {
                    fields.push(std::mem::take(&mut field));
                    state = State::Start;
                }
                '"' | '\r' | '\n' => return None,
                _ => field.push(character),
            },
            State::Quoted => match character {
                '"' => {
                    if characters.peek() == Some(&'"') {
                        characters.next();
                        field.push('"');
                    } else {
                        state = State::AfterQuote;
                    }
                }
                '\r' | '\n' => return None,
                _ => field.push(character),
            },
            State::AfterQuote => match character {
                ',' => {
                    fields.push(std::mem::take(&mut field));
                    state = State::Start;
                }
                _ => return None,
            },
        }
    }
    if matches!(state, State::Quoted) {
        return None;
    }
    fields.push(field);
    if fields.len() < 3 || fields[3..].iter().any(|field| !field.is_empty()) {
        return None;
    }
    fields.into_iter().next().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::adapters::package_record;
    use crate::core::{Ecosystem, ResourceKind};

    use super::*;

    #[cfg(unix)]
    #[test]
    fn hard_links_are_counted_once_and_symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        fs::write(&real, b"1234").unwrap();
        fs::hard_link(&real, directory.path().join("hard")).unwrap();
        symlink(&real, directory.path().join("link")).unwrap();

        assert_eq!(
            measure_paths(&[directory.path().to_path_buf()])
                .unwrap()
                .bytes,
            4
        );
    }

    #[test]
    fn npm_resolution_never_measures_a_sibling_package() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("a")).unwrap();
        fs::create_dir(directory.path().join("b")).unwrap();
        fs::write(directory.path().join("a/data"), vec![0; 4]).unwrap();
        fs::write(directory.path().join("b/data"), vec![0; 8]).unwrap();
        let package = package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "a",
            Some("1.0.0".into()),
            None,
        );

        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);

        assert_eq!(measure_paths(&paths.paths).unwrap().bytes, 4);
        assert_eq!(paths.paths, vec![directory.path().join("a")]);
    }

    #[test]
    fn rustup_components_and_targets_are_not_counted_again() {
        let root = PathBuf::from("/tmp/rustup/toolchains");
        for kind in [ResourceKind::Component, ResourceKind::Target] {
            let package = package_record(Ecosystem::Rustup, kind, "component:rustfmt", None, None);
            let paths = resolve_package_paths(&package, std::slice::from_ref(&root));
            assert!(!paths.measurable);
            assert!(paths.paths.is_empty());
        }
    }

    #[test]
    fn pip_uses_record_files_instead_of_the_site_packages_root() {
        let directory = tempfile::tempdir().unwrap();
        let metadata = directory.path().join("demo_pkg-1.0.dist-info");
        fs::create_dir(&metadata).unwrap();
        fs::create_dir(directory.path().join("demo_pkg")).unwrap();
        fs::create_dir(directory.path().join("other_pkg")).unwrap();
        fs::write(metadata.join("METADATA"), "Name: demo-pkg\n").unwrap();
        fs::write(
            metadata.join("RECORD"),
            "demo_pkg/data.py,,\ndemo_pkg-1.0.dist-info/METADATA,,\n",
        )
        .unwrap();
        fs::write(directory.path().join("demo_pkg/data.py"), b"1234").unwrap();
        fs::write(directory.path().join("other_pkg/data.py"), vec![0; 64]).unwrap();
        let package = package_record(
            Ecosystem::Pip,
            ResourceKind::Package,
            "demo-pkg",
            Some("1.0".into()),
            None,
        );

        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);
        let usage = measure_paths(&paths.paths).unwrap();
        let expected = fs::metadata(metadata.join("METADATA")).unwrap().len()
            + fs::metadata(metadata.join("RECORD")).unwrap().len()
            + fs::metadata(directory.path().join("demo_pkg/data.py"))
                .unwrap()
                .len();

        assert!(paths.measurable);
        assert_eq!(usage.bytes, expected);
        assert!(!paths.paths.contains(&directory.path().to_path_buf()));
    }

    #[test]
    fn pip_record_cannot_add_an_absolute_directory() {
        let directory = tempfile::tempdir().unwrap();
        let metadata = directory.path().join("demo_pkg-1.0.dist-info");
        fs::create_dir(&metadata).unwrap();
        fs::write(metadata.join("METADATA"), "Name: demo-pkg\n").unwrap();
        fs::write(metadata.join("RECORD"), "/,,\n").unwrap();
        let package = package_record(
            Ecosystem::Pip,
            ResourceKind::Package,
            "demo-pkg",
            Some("1.0".into()),
            None,
        );

        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);

        assert!(!paths.measurable);
        assert!(paths.paths.is_empty());
    }

    #[test]
    fn pip_without_a_readable_record_is_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let metadata = directory.path().join("demo_pkg-1.0.dist-info");
        fs::create_dir(&metadata).unwrap();
        fs::write(metadata.join("METADATA"), "Name: demo-pkg\n").unwrap();
        let package = package_record(
            Ecosystem::Pip,
            ResourceKind::Package,
            "demo-pkg",
            Some("1.0".into()),
            None,
        );

        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);

        assert!(!paths.measurable);
        assert!(paths.paths.is_empty());
    }

    #[test]
    fn pip_record_rejects_malformed_csv_rows() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("demo_pkg")).unwrap();
        fs::write(directory.path().join("demo_pkg/data.py"), b"1234").unwrap();
        for row in [
            "demo_pkg/data.py",
            "demo_pkg/data.py,",
            "\"demo_pkg/data.py,,",
            "\"demo_pkg/data.py\"unterminated,,",
            "demo_pkg/data.py,,,unexpected",
        ] {
            assert!(
                complete_record_paths(directory.path(), row).is_none(),
                "malformed RECORD row was accepted: {row:?}"
            );
        }
    }

    #[test]
    fn pip_record_accepts_quoted_paths_and_escaped_quotes() {
        let directory = tempfile::tempdir().unwrap();
        let relative = "demo_pkg/data,\"quoted\".py";
        let path = directory.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"1234").unwrap();

        let row = "\"demo_pkg/data,\"\"quoted\"\".py\",,";
        assert_eq!(
            complete_record_paths(directory.path(), row),
            Some(vec![path])
        );
    }

    #[test]
    fn unchanged_signature_returns_the_cached_measurement() {
        let directory = tempfile::tempdir().unwrap();
        let package_root = directory.path().join("demo");
        fs::create_dir(&package_root).unwrap();
        fs::write(package_root.join("data"), b"1234").unwrap();
        let package = package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "demo",
            Some("1.0.0".into()),
            None,
        );
        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);
        let cache = CacheStore::in_memory();
        let service = DiskUsageService::new(cache.clone());
        let first = service.measure(&package, &paths).unwrap();
        let cached = cache.get_for(&package, &paths.install_root_key()).unwrap();

        let second = service
            .measure_if_needed(&package, &paths, cached, &CancellationToken::new())
            .unwrap();

        assert_eq!(second, first);
    }

    #[test]
    fn changed_installed_version_requires_measurement() {
        let directory = tempfile::tempdir().unwrap();
        let package_root = directory.path().join("demo");
        fs::create_dir(&package_root).unwrap();
        fs::write(package_root.join("data"), b"1234").unwrap();
        let package = package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "demo",
            Some("1.0.0".into()),
            None,
        );
        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);
        let cache = CacheStore::in_memory();
        let service = DiskUsageService::new(cache.clone());
        service.measure(&package, &paths).unwrap();
        let cached = cache.get_for(&package, &paths.install_root_key()).unwrap();
        let updated = package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "demo",
            Some("2.0.0".into()),
            None,
        );

        assert!(service
            .should_measure(&updated, &paths, cached.as_ref())
            .unwrap());
    }

    #[test]
    fn changed_install_root_requires_measurement() {
        let directory = tempfile::tempdir().unwrap();
        let old_root = directory.path().join("old");
        let new_root = directory.path().join("new");
        fs::create_dir_all(old_root.join("demo")).unwrap();
        fs::create_dir_all(new_root.join("demo")).unwrap();
        let package = package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "demo",
            Some("1.0.0".into()),
            None,
        );
        let old_paths = resolve_package_paths(&package, std::slice::from_ref(&old_root));
        let new_paths = resolve_package_paths(&package, std::slice::from_ref(&new_root));
        let cache = CacheStore::in_memory();
        let service = DiskUsageService::new(cache.clone());
        service.measure(&package, &old_paths).unwrap();
        let cached = cache
            .get_for(&package, &old_paths.install_root_key())
            .unwrap();

        assert!(service
            .should_measure(&package, &new_paths, cached.as_ref())
            .unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn replacing_the_same_path_with_a_new_inode_requires_measurement() {
        let directory = tempfile::tempdir().unwrap();
        let package_root = directory.path().join("demo");
        fs::create_dir(&package_root).unwrap();
        fs::write(package_root.join("data"), b"1234").unwrap();
        let package = package_record(
            Ecosystem::Npm,
            ResourceKind::Package,
            "demo",
            Some("1.0.0".into()),
            None,
        );
        let paths = resolve_package_paths(&package, &[directory.path().to_path_buf()]);
        let cache = CacheStore::in_memory();
        let service = DiskUsageService::new(cache.clone());
        service.measure(&package, &paths).unwrap();
        let cached = cache.get_for(&package, &paths.install_root_key()).unwrap();
        fs::rename(&package_root, directory.path().join("old-demo")).unwrap();
        fs::create_dir(&package_root).unwrap();
        fs::write(package_root.join("data"), b"5678").unwrap();

        assert!(service
            .should_measure(&package, &paths, cached.as_ref())
            .unwrap());
    }
}
