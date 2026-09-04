//! 统一的子进程执行与输出脱敏入口。

mod process;
mod redaction;

pub use process::{
    sink, CommandResult, CommandSpec, EventSink, OutputEvent, ProcessError, ProcessSupervisor,
};
pub use redaction::Redactor;
