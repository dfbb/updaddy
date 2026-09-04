mod adapter;
mod gem;
mod homebrew;
mod npm;
mod parsers;
mod pip;
mod rustup;

pub use adapter::{
    classify_command_error, command, package_record, validate_name, CommandRunner,
    EcosystemAdapter, ExecutorContext, ProcessCommandRunner, ProxyEnv,
};
pub use gem::GemAdapter;
pub use homebrew::HomebrewAdapter;
pub use npm::NpmAdapter;
pub use pip::PipAdapter;
pub use rustup::RustupAdapter;
