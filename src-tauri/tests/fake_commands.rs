use updaddy_lib::adapters::{
    EcosystemAdapter, GemAdapter, HomebrewAdapter, NpmAdapter, PipAdapter, RustupAdapter,
};
use updaddy_lib::core::{Ecosystem, Operation, PackageTask};

#[test]
fn npm_update_is_global_and_argv_parameterized() {
    let task = PackageTask::new(Ecosystem::Npm, "typescript", Operation::Update);
    let spec = NpmAdapter::new().plan(&task).unwrap();
    assert_eq!(spec.program, "npm");
    assert_eq!(
        spec.args.iter().map(String::as_str).collect::<Vec<_>>(),
        ["install", "--global", "typescript"]
    );
}

#[test]
fn pip_uninstall_never_uses_shell_or_sudo() {
    let task = PackageTask::new(Ecosystem::Pip, "requests", Operation::Uninstall);
    let spec = PipAdapter::new().plan(&task).unwrap();
    assert_eq!(spec.program, "python");
    assert_eq!(
        spec.args.iter().map(String::as_str).collect::<Vec<_>>(),
        ["-m", "pip", "uninstall", "--yes", "requests"]
    );
    assert!(!spec
        .args
        .iter()
        .any(|arg| arg == "sudo" || arg.contains(";")));
}

#[test]
fn homebrew_cask_and_tap_use_dedicated_commands() {
    let cask = PackageTask::new(Ecosystem::Homebrew, "cask:firefox", Operation::Uninstall);
    assert_eq!(
        HomebrewAdapter::new()
            .plan(&cask)
            .unwrap()
            .args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["uninstall", "--cask", "firefox"]
    );
    let tap = PackageTask::new(Ecosystem::Homebrew, "tap:acme/tools", Operation::Uninstall);
    assert_eq!(
        HomebrewAdapter::new()
            .plan(&tap)
            .unwrap()
            .args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["untap", "acme/tools"]
    );
}

#[test]
fn gem_version_and_rustup_resource_are_argv_values() {
    let gem = PackageTask::new(Ecosystem::Gem, "rails@7.1.0", Operation::Uninstall);
    assert_eq!(
        GemAdapter::new()
            .plan(&gem)
            .unwrap()
            .args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["uninstall", "rails", "--version", "7.1.0"]
    );
    let target = PackageTask::new(
        Ecosystem::Rustup,
        "target:wasm32-unknown-unknown",
        Operation::Uninstall,
    );
    assert_eq!(
        RustupAdapter::new()
            .plan(&target)
            .unwrap()
            .args
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["target", "remove", "wasm32-unknown-unknown"]
    );
}
