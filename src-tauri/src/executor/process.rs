use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::time;
use tokio_util::sync::CancellationToken;

use super::Redactor;

pub type EventSink = Arc<dyn Fn(OutputEvent) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputEvent {
    pub emitted_at: i64,
    pub stream: String,
    pub text: String,
}

/// The command contract shared by all ecosystem adapters.
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
}

impl CommandSpec {
    pub fn for_test(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: HashMap::new(),
            cwd: None,
            stdin: None,
        }
    }
}

#[derive(Debug)]
pub struct CommandResult {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to spawn command: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed while reading command output: {0}")]
    Io(#[source] io::Error),
    #[error("command exceeded the process timeout")]
    ProcessTimeout,
    #[error("command was cancelled by the user")]
    Cancelled,
    #[error("command exited unsuccessfully: {0}")]
    CommandFailed(ExitStatus),
}

impl ProcessError {
    pub fn is_retryable(&self) -> bool {
        false
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }
}

pub struct ProcessSupervisor;

impl ProcessSupervisor {
    pub async fn run(
        spec: CommandSpec,
        cancel_token: CancellationToken,
        event_sink: EventSink,
    ) -> Result<CommandResult, ProcessError> {
        Self::run_with_timeout(spec, cancel_token, event_sink, Duration::from_secs(300)).await
    }

    pub async fn run_with_timeout(
        spec: CommandSpec,
        cancel_token: CancellationToken,
        event_sink: EventSink,
        timeout: Duration,
    ) -> Result<CommandResult, ProcessError> {
        let started = std::time::Instant::now();
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .env_clear()
            .envs(&spec.env)
            .env("TERM", "dumb")
            .current_dir(
                spec.cwd.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                }),
            )
            .stdin(if spec.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "macos")]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(ProcessError::Spawn)?;
        if let Some(input) = spec.stdin {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                stdin.write_all(&input).await.map_err(ProcessError::Io)?;
            }
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io(io::Error::other("stdout unavailable")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProcessError::Io(io::Error::other("stderr unavailable")))?;
        let stdout_task = tokio::spawn(async move { read_output(stdout).await });
        let stderr_task = tokio::spawn(async move { read_output(stderr).await });

        let status = tokio::select! {
            status = child.wait() => status.map_err(ProcessError::Io)?,
            _ = cancel_token.cancelled() => {
                terminate_process_group(&mut child).await;
                return Err(ProcessError::Cancelled);
            }
            _ = time::sleep(timeout) => {
                terminate_process_group(&mut child).await;
                return Err(ProcessError::ProcessTimeout);
            }
        };

        let stdout = stdout_task
            .await
            .map_err(|e| ProcessError::Io(io::Error::other(e)))??;
        let stderr = stderr_task
            .await
            .map_err(|e| ProcessError::Io(io::Error::other(e)))??;
        let secret_refs: Vec<&str> = spec
            .env
            .values()
            .filter(|value| !value.is_empty())
            .map(String::as_str)
            .collect();
        let stdout = redact_output(&stdout, "stdout", &event_sink, &secret_refs);
        let stderr = redact_output(&stderr, "stderr", &event_sink, &secret_refs);

        Ok(CommandResult {
            status,
            stdout,
            stderr,
            duration: started.elapsed(),
        })
    }
}

async fn read_output<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> Result<String, io::Error> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn redact_output(text: &str, stream: &str, event_sink: &EventSink, secrets: &[&str]) -> String {
    let redacted = Redactor::redact(text, secrets);
    for line in redacted.lines() {
        event_sink(OutputEvent {
            emitted_at: Utc::now().timestamp(),
            stream: stream.to_owned(),
            text: line.to_owned(),
        });
    }
    redacted
}

async fn terminate_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            #[cfg(target_os = "macos")]
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = child.start_kill();
            }
            if time::timeout(Duration::from_secs(3), child.wait())
                .await
                .is_err()
            {
                #[cfg(target_os = "macos")]
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
                let _ = child.kill().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
}

pub fn sink() -> EventSink {
    Arc::new(|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_result_captures_stdout_and_exit_code() {
        let result = ProcessSupervisor::run(
            CommandSpec::for_test("printf", &["ok"]),
            CancellationToken::new(),
            sink(),
        )
        .await
        .unwrap();
        assert_eq!(result.stdout, "ok");
        assert_eq!(result.status.code(), Some(0));
    }

    #[tokio::test]
    async fn timeout_is_not_retryable() {
        let result = ProcessSupervisor::run_with_timeout(
            CommandSpec::for_test("sleep", &["2"]),
            CancellationToken::new(),
            sink(),
            Duration::from_millis(20),
        )
        .await;
        assert!(matches!(result, Err(ProcessError::ProcessTimeout)));
    }
}
