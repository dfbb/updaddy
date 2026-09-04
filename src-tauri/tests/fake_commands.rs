use updaddy_lib::adapters::{EcosystemAdapter, NpmAdapter, PipAdapter};
use updaddy_lib::core::{Ecosystem, Operation, PackageTask};

#[test]
fn npm_update_is_global_and_argv_parameterized() {
    let task = PackageTask::new(Ecosystem::Npm, "typescript", Operation::Update);
    let spec = NpmAdapter::new().plan(&task).unwrap();
    assert_eq!(spec.program, "npm");
    assert_eq!(spec.args, ["install", "--global", "typescript"]);
}

#[test]
fn pip_uninstall_never_uses_shell_or_sudo() {
    let task = PackageTask::new(Ecosystem::Pip, "requests", Operation::Uninstall);
    let spec = PipAdapter::new().plan(&task).unwrap();
    assert_eq!(spec.program, "python");
    assert_eq!(spec.args, ["-m", "pip", "uninstall", "--yes", "requests"]);
    assert!(!spec
        .args
        .iter()
        .any(|arg| arg == "sudo" || arg.contains(";")));
}
