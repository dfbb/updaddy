mod adapter;
mod gem;
mod homebrew;
mod npm;
mod parsers;
mod pip;
mod rustup;

use crate::core::Ecosystem;
use std::collections::HashMap;
use std::sync::Arc;

pub use adapter::{
    classify_command_error, classify_process_error, command, detect_process_error, home_dir,
    home_path, package_record, validate_name, validate_resource_name, CommandRunner,
    EcosystemAdapter, ExecutorContext, ProcessCommandRunner, ProxyEnv,
};
pub use gem::GemAdapter;
pub use homebrew::HomebrewAdapter;
pub(crate) use homebrew::{HomebrewProgressPhase, HomebrewProgressTracker};
pub use npm::NpmAdapter;
pub use pip::PipAdapter;
pub use rustup::RustupAdapter;

/// Construct the five production adapters for WorkerSupervisor registration.
pub fn default_adapters() -> HashMap<Ecosystem, Arc<dyn EcosystemAdapter>> {
    HashMap::from([
        (
            Ecosystem::Homebrew,
            Arc::new(HomebrewAdapter::new()) as Arc<dyn EcosystemAdapter>,
        ),
        (
            Ecosystem::Npm,
            Arc::new(NpmAdapter::new()) as Arc<dyn EcosystemAdapter>,
        ),
        (
            Ecosystem::Pip,
            Arc::new(PipAdapter::new()) as Arc<dyn EcosystemAdapter>,
        ),
        (
            Ecosystem::Gem,
            Arc::new(GemAdapter::new()) as Arc<dyn EcosystemAdapter>,
        ),
        (
            Ecosystem::Rustup,
            Arc::new(RustupAdapter::new()) as Arc<dyn EcosystemAdapter>,
        ),
    ])
}
