use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::Redactor;

pub type EventSink = Arc<dyn Fn(OutputEvent) + Send + Sync + 'static>;
pub type OutputChunkSink = Arc<dyn Fn(&[u8]) + Send + Sync + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputEvent {
    pub task_id: Option<Uuid>,
    pub emitted_at: i64,
    pub stream: String,
    pub text: String,
}

/// The command contract shared by all ecosystem adapters.
#[derive(Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Duration,
    pub task_id: Option<Uuid>,
    pub sudo: bool,
    pub pseudo_terminal: bool,
    pub(crate) output_chunk_sink: Option<OutputChunkSink>,
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env = self
            .env
            .keys()
            .map(|key| (key.as_str(), "<redacted>"))
            .collect::<HashMap<_, _>>();
        formatter
            .debug_struct("CommandSpec")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &env)
            .field("cwd", &self.cwd)
            .field(
                "stdin",
                &self.stdin.as_ref().map(|v| format!("<{} bytes>", v.len())),
            )
            .field("timeout", &self.timeout)
            .field("task_id", &self.task_id)
            .field("sudo", &self.sudo)
            .field("pseudo_terminal", &self.pseudo_terminal)
            .field("has_output_chunk_sink", &self.output_chunk_sink.is_some())
            .finish()
    }
}

impl CommandSpec {
    pub fn for_test(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: HashMap::new(),
            cwd: None,
            stdin: None,
            timeout: Duration::from_secs(300),
            task_id: None,
            sudo: false,
            pseudo_terminal: false,
            output_chunk_sink: None,
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
    #[error("proxy error: {0}")]
    Proxy(#[source] crate::proxy::ProxyError),
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
        let timeout = spec.timeout;
        Self::run_with_timeout(spec, cancel_token, event_sink, timeout).await
    }

    pub async fn run_with_timeout(
        spec: CommandSpec,
        cancel_token: CancellationToken,
        event_sink: EventSink,
        timeout: Duration,
    ) -> Result<CommandResult, ProcessError> {
        let started = std::time::Instant::now();
        let task_id = spec.task_id;
        let pseudo_terminal = spec.pseudo_terminal;
        let failure_event_sink = event_sink.clone();
        let secrets = sensitive_env_values(&spec.env);
        if task_id.is_some() {
            let secret_refs = secrets.iter().map(String::as_str).collect::<Vec<_>>();
            event_sink(OutputEvent {
                task_id,
                emitted_at: Utc::now().timestamp(),
                stream: "command".into(),
                text: Redactor::redact(&format_command(&spec), &secret_refs),
            });
        }
        let mut command = if spec.pseudo_terminal && cfg!(target_os = "macos") {
            let mut command = Command::new("/usr/bin/script");
            command.args(["-q", "/dev/null", &spec.program]);
            command.args(&spec.args);
            command
        } else if spec.sudo && cfg!(target_os = "macos") {
            let mut command = Command::new("/usr/bin/sudo");
            command.args(["-A", "-E", "--", &spec.program]);
            command.args(&spec.args);
            command
        } else {
            let mut command = Command::new(&spec.program);
            command.args(&spec.args);
            command
        };
        command
            .env_clear()
            .envs(&spec.env)
            .env(
                "TERM",
                if spec.pseudo_terminal {
                    "xterm-256color"
                } else {
                    "dumb"
                },
            )
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

        // `sudo -A` starts the askpass helper as a child process. Preserve the
        // current user's minimal macOS session environment so osascript can
        // return the dialog value to sudo after env_clear above.
        if spec.sudo && cfg!(target_os = "macos") {
            for key in ["HOME", "USER", "LOGNAME", "PATH", "__CF_USER_TEXT_ENCODING"] {
                if let Ok(value) = std::env::var(key) {
                    command.env(key, value);
                }
            }
        }

        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(ProcessError::Spawn)?;
        let stdin_task = spec.stdin.and_then(|input| {
            child.stdin.take().map(|mut stdin| {
                tokio::spawn(async move { stdin.write_all(&input).await.map_err(ProcessError::Io) })
            })
        });

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ProcessError::Io(io::Error::other("stdout unavailable")))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ProcessError::Io(io::Error::other("stderr unavailable")))?;
        let result_secrets = secrets.clone();
        let (activity_tx, mut activity_rx) = mpsc::unbounded_channel();
        let stdout_task = tokio::spawn(read_output(
            stdout,
            "stdout",
            event_sink.clone(),
            secrets.clone(),
            spec.output_chunk_sink.clone(),
            spec.task_id,
            Some(activity_tx.clone()),
        ));
        let stderr_task = tokio::spawn(read_output(
            stderr,
            "stderr",
            event_sink,
            secrets,
            spec.output_chunk_sink,
            spec.task_id,
            Some(activity_tx),
        ));

        let status = if pseudo_terminal {
            // Homebrew downloads may legitimately exceed the nominal command
            // duration. Reset the idle timer whenever either output stream has
            // activity, while still terminating a genuinely stuck process.
            let pid = child.id();
            let wait = child.wait();
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    status = &mut wait => break status.map_err(ProcessError::Io)?,
                    _ = cancel_token.cancelled() => {
                        // Homebrew treats SIGINT as a cooperative cancellation: its
                        // download queue forwards the signal to curl and deliberately
                        // keeps the `.incomplete` file so a later invocation can resume.
                        // SIGTERM skips that cleanup path and can discard the partial
                        // download.  Give the process a short grace period before the
                        // hard kill fallback.
                        terminate_process_group_id_graceful(pid).await;
                        if let Some(task) = stdin_task { task.abort(); }
                        let _ = tokio::join!(reap_reader(stdout_task), reap_reader(stderr_task));
                        return Err(ProcessError::Cancelled);
                    }
                    _ = time::sleep(timeout) => {
                        // Preserve Homebrew's resumable-download semantics on timeout as
                        // well as on an explicit cancellation.
                        terminate_process_group_id_graceful(pid).await;
                        if let Some(task) = stdin_task { task.abort(); }
                        let _ = tokio::join!(reap_reader(stdout_task), reap_reader(stderr_task));
                        emit_failure_summary(task_id, "command exceeded the process idle timeout", &failure_event_sink);
                        return Err(ProcessError::ProcessTimeout);
                    }
                    activity = activity_rx.recv() => {
                        if activity.is_none() {
                            // Both readers exited; child.wait remains the source
                            // of truth for the process lifecycle.
                        }
                    }
                }
            }
        } else {
            tokio::select! {
                status = child.wait() => status.map_err(ProcessError::Io)?,
                _ = cancel_token.cancelled() => {
                    terminate_process_group(&mut child).await;
                    if let Some(task) = stdin_task { task.abort(); }
                    let _ = tokio::join!(reap_reader(stdout_task), reap_reader(stderr_task));
                    return Err(ProcessError::Cancelled);
                }
                _ = time::sleep(timeout) => {
                    terminate_process_group(&mut child).await;
                    if let Some(task) = stdin_task { task.abort(); }
                    let _ = tokio::join!(reap_reader(stdout_task), reap_reader(stderr_task));
                    emit_failure_summary(task_id, "command exceeded the process timeout", &failure_event_sink);
                    return Err(ProcessError::ProcessTimeout);
                }
            }
        };

        if let Some(task) = stdin_task {
            task.abort();
        }
        let (stdout, stderr) =
            tokio::try_join!(reap_reader(stdout_task), reap_reader(stderr_task))?;
        let secret_refs: Vec<&str> = result_secrets.iter().map(String::as_str).collect();
        let stdout = Redactor::redact(&stdout, &secret_refs);
        let stderr = Redactor::redact(&stderr, &secret_refs);
        if pseudo_terminal && !status.success() {
            let diagnostic = if stderr.trim().is_empty() {
                &stdout
            } else {
                &stderr
            };
            emit_failure_summary(task_id, diagnostic, &failure_event_sink);
        }

        Ok(CommandResult {
            status,
            stdout,
            stderr,
            duration: started.elapsed(),
        })
    }
}

fn is_sensitive_env_key(key: &str) -> bool {
    let key = key.to_ascii_uppercase();
    crate::proxy::is_proxy_env_key(&key)
        || ["PASSWORD", "TOKEN", "SECRET", "CREDENTIAL", "AUTH"]
            .iter()
            .any(|marker| key.contains(marker))
}

fn sensitive_env_values(env: &HashMap<String, String>) -> Vec<String> {
    env.iter()
        .filter(|(key, value)| !value.is_empty() && is_sensitive_env_key(key))
        .map(|(_, value)| value.clone())
        .collect()
}

fn format_command(spec: &CommandSpec) -> String {
    let mut parts = if spec.sudo && cfg!(target_os = "macos") {
        vec![
            "sudo".to_owned(),
            "-A".to_owned(),
            "-E".to_owned(),
            "--".to_owned(),
        ]
    } else {
        Vec::new()
    };
    parts.push(spec.program.clone());
    parts.extend(spec.args.clone());
    format!(
        "$ {}",
        parts
            .iter()
            .map(|part| shell_quote(part))
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:@=".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn emit_failure_summary(task_id: Option<Uuid>, output: &str, event_sink: &EventSink) {
    let Some(task_id) = task_id else {
        return;
    };
    let output = sanitize_terminal_output(output);
    let output = output.trim();
    if output.is_empty() {
        return;
    }
    const MAX_DIAGNOSTIC_BYTES: usize = 128 * 1024;
    let start = output.len().saturating_sub(MAX_DIAGNOSTIC_BYTES);
    let start = (start..output.len())
        .find(|index| output.is_char_boundary(*index))
        .unwrap_or(0);
    event_sink(OutputEvent {
        task_id: Some(task_id),
        emitted_at: Utc::now().timestamp(),
        stream: "error".into(),
        text: output[start..].to_owned(),
    });
}

fn sanitize_terminal_output(output: &str) -> String {
    let mut sanitized = String::with_capacity(output.len());
    let mut chars = output.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    // CSI sequence, for example color, cursor movement, or
                    // erase-line controls.
                    for control in chars.by_ref() {
                        if ('@'..='~').contains(&control) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC sequence (titles/hyperlinks), terminated by BEL or
                    // ST (ESC backslash).
                    while let Some(control) = chars.next() {
                        if control == '\u{7}' {
                            break;
                        }
                        if control == '\u{1b}' {
                            chars.next_if_eq(&'\\');
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            }
            continue;
        }
        if character == '\r' {
            sanitized.push('\n');
            chars.next_if_eq(&'\n');
        } else if character == '\n' || character == '\t' || !character.is_control() {
            sanitized.push(character);
        }
    }
    sanitized
}

async fn read_output<R: AsyncRead + Unpin>(
    mut reader: R,
    stream: &str,
    event_sink: EventSink,
    secrets: Vec<String>,
    output_chunk_sink: Option<OutputChunkSink>,
    task_id: Option<Uuid>,
    activity_sink: Option<mpsc::UnboundedSender<()>>,
) -> Result<String, io::Error> {
    let mut bytes = Vec::new();
    let mut pending = Vec::new();
    let has_multiline_secret = secrets
        .iter()
        .any(|secret| secret.contains('\n') || secret.contains('\r'));
    let mut chunk = [0_u8; 4096];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        if let Some(activity) = &activity_sink {
            let _ = activity.send(());
        }
        if let Some(sink) = &output_chunk_sink {
            // Chunk consumers derive numeric progress only; raw bytes never leave the backend.
            sink(&chunk[..count]);
        }
        bytes.extend_from_slice(&chunk[..count]);
        pending.extend_from_slice(&chunk[..count]);
        if !has_multiline_secret {
            while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                let mut line = pending.drain(..=newline).collect::<Vec<_>>();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let line = String::from_utf8_lossy(&line);
                emit_line(&line, stream, &event_sink, &secrets, task_id);
            }
        }
    }
    if has_multiline_secret {
        // A secret may span lines; buffer this stream until EOF so no partial event leaks it.
        let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let redacted = Redactor::redact(&String::from_utf8_lossy(&bytes), &secret_refs);
        for line in redacted.lines() {
            emit_line(line, stream, &event_sink, &[], task_id);
        }
    } else if !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending);
        emit_line(&line, stream, &event_sink, &secrets, task_id);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn reap_reader(
    mut task: JoinHandle<Result<String, io::Error>>,
) -> Result<String, ProcessError> {
    match time::timeout(Duration::from_secs(3), &mut task).await {
        Ok(joined) => joined
            .map_err(|error| ProcessError::Io(io::Error::other(error)))?
            .map_err(ProcessError::Io),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err(ProcessError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "output reader cleanup timed out",
            )))
        }
    }
}

fn emit_line(
    line: &str,
    stream: &str,
    event_sink: &EventSink,
    secrets: &[String],
    task_id: Option<Uuid>,
) {
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let redacted = Redactor::redact(line, &secret_refs);
    event_sink(OutputEvent {
        task_id,
        emitted_at: Utc::now().timestamp(),
        stream: stream.to_owned(),
        text: sanitize_terminal_output(&redacted),
    });
}

async fn terminate_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            let _ = time::timeout(Duration::from_secs(3), child.wait()).await;
            // The group may already be gone; SIGKILL is intentionally unconditional.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = time::timeout(Duration::from_secs(1), child.kill()).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = time::timeout(Duration::from_secs(1), child.kill()).await;
    }
}

async fn terminate_process_group_id_graceful(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // Homebrew's SystemCommand rescue path expects SIGINT. It forwards INT to
        // curl, which leaves the partial `.incomplete` artifact for --continue-at.
        unsafe { libc::kill(-(pid as i32), libc::SIGINT) };
        // Allow brew/curl enough time to flush and close the partial file. Sending
        // KILL immediately after INT can interrupt that cleanup and lose resume state.
        for _ in 0..60 {
            let still_running = unsafe { libc::kill(-(pid as i32), 0) } == 0;
            if !still_running {
                return;
            }
            time::sleep(Duration::from_millis(50)).await;
        }
        // A command that ignores INT still must not outlive the cancelled task.
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
}

pub fn sink() -> EventSink {
    Arc::new(|_| {})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spec_debug_redacts_proxy_environment() {
        let mut spec = CommandSpec::for_test("tool", &[]);
        spec.env.insert(
            "ALL_PROXY".into(),
            "socks5://alice:secret@example.test:1080".into(),
        );
        let rendered = format!("{spec:?}");
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("socks5://"));
    }

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
    async fn command_result_preserves_paths_but_redacts_sensitive_environment() {
        let mut spec =
            CommandSpec::for_test("sh", &["-c", "printf '%s|%s' \"$HOME\" \"$HTTPS_PROXY\""]);
        spec.env.insert("HOME".into(), "/tmp/updaddy-home".into());
        spec.env.insert(
            "HTTPS_PROXY".into(),
            "http://user:password@127.0.0.1:8080".into(),
        );

        let result = ProcessSupervisor::run(spec, CancellationToken::new(), sink())
            .await
            .unwrap();

        assert_eq!(result.stdout, "/tmp/updaddy-home|[REDACTED]");
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

    #[tokio::test]
    async fn pseudo_terminal_timeout_is_reset_by_output_activity() {
        let mut spec = CommandSpec::for_test(
            "sh",
            &["-c", "for i in 1 2 3; do printf x; sleep 0.03; done"],
        );
        spec.pseudo_terminal = true;
        let result = ProcessSupervisor::run_with_timeout(
            spec,
            CancellationToken::new(),
            sink(),
            Duration::from_millis(50),
        )
        .await;
        assert!(
            result.is_ok(),
            "continuous output must prevent idle timeout"
        );
    }

    #[tokio::test]
    async fn pseudo_terminal_cancellation_sends_interrupt_before_kill() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.cancel();
        });
        let mut spec = CommandSpec::for_test(
            "sh",
            &["-c", "trap 'printf interrupted; exit 130' INT; sleep 10"],
        );
        spec.pseudo_terminal = true;
        let result =
            ProcessSupervisor::run_with_timeout(spec, token, sink(), Duration::from_secs(5)).await;
        assert!(matches!(result, Err(ProcessError::Cancelled)));
    }

    #[tokio::test]
    async fn cancellation_does_not_wait_for_blocked_stdin_writer() {
        let token = CancellationToken::new();
        let cancel = token.clone();
        let cancel_task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        });
        let mut spec = CommandSpec::for_test("sh", &["-c", "sleep 10"]);
        spec.stdin = Some(vec![b'x'; 8 * 1024 * 1024]);
        let started = std::time::Instant::now();
        let result =
            ProcessSupervisor::run_with_timeout(spec, token, sink(), Duration::from_secs(5)).await;
        cancel_task.await.unwrap();
        assert!(matches!(result, Err(ProcessError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn output_events_are_emitted_per_line() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = events.clone();
        let event_sink: EventSink = Arc::new(move |event| {
            collected.lock().unwrap().push(event);
        });
        let task_id = Uuid::new_v4();
        let mut spec = CommandSpec::for_test("printf", &["one\\ntwo\\n"]);
        spec.task_id = Some(task_id);
        ProcessSupervisor::run(spec, CancellationToken::new(), event_sink)
            .await
            .unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events[0].stream, "command");
        assert_eq!(events[0].text, "$ printf 'one\\ntwo\\n'");
        assert_eq!(
            events
                .iter()
                .filter(|event| event.stream == "stdout")
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert!(events.iter().all(|event| event.task_id == Some(task_id)));
    }

    #[tokio::test]
    async fn output_chunks_are_available_before_a_trailing_newline() {
        let chunks = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = chunks.clone();
        let mut spec = CommandSpec::for_test("printf", &["10.0MB/100.0MB"]);
        spec.pseudo_terminal = true;
        spec.output_chunk_sink = Some(Arc::new(move |chunk| {
            collected.lock().unwrap().extend_from_slice(chunk);
        }));

        ProcessSupervisor::run(spec, CancellationToken::new(), sink())
            .await
            .unwrap();

        assert!(String::from_utf8_lossy(&chunks.lock().unwrap()).contains("10.0MB/100.0MB"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn failed_pseudo_terminal_emits_readable_task_error_output() {
        let task_id = Uuid::new_v4();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = events.clone();
        let event_sink: EventSink = Arc::new(move |event| {
            collected.lock().unwrap().push(event);
        });
        let mut spec = CommandSpec::for_test(
            "sh",
            &["-c", "printf '\\033[31mbrew failed\\033[0m\\n'; exit 1"],
        );
        spec.task_id = Some(task_id);
        spec.pseudo_terminal = true;

        let result = ProcessSupervisor::run(spec, CancellationToken::new(), event_sink)
            .await
            .unwrap();

        assert!(!result.status.success());
        assert!(events.lock().unwrap().iter().any(|event| {
            event.task_id == Some(task_id)
                && event.stream == "error"
                && event.text.contains("brew failed")
                && !event.text.contains('\u{1b}')
        }));
    }

    #[test]
    fn terminal_diagnostic_removes_control_characters_and_normalizes_newlines() {
        assert_eq!(
            sanitize_terminal_output("\u{4}\u{8}\u{8}Warning\r\nDetails\n"),
            "Warning\nDetails\n"
        );
        assert_eq!(
            sanitize_terminal_output("\u{1b}[32mOK\u{1b}[0m\u{1b}]0;title\u{7}\rDone"),
            "OK\nDone"
        );
    }

    #[tokio::test]
    async fn output_reader_preserves_utf8_split_across_chunks() {
        let (mut writer, reader) = tokio::io::duplex(16);
        let write_task = tokio::spawn(async move {
            writer.write_all(&[b'c', b'a', b'f', 0xc3]).await.unwrap();
            writer.write_all(&[0xa9, b'\n']).await.unwrap();
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = events.clone();
        let event_sink: EventSink = Arc::new(move |event| {
            collected.lock().unwrap().push(event);
        });
        let output = read_output(reader, "stdout", event_sink, Vec::new(), None, None, None)
            .await
            .unwrap();
        write_task.await.unwrap();
        assert_eq!(output, "café\n");
        assert_eq!(events.lock().unwrap()[0].text, "café");
    }

    #[tokio::test]
    async fn multiline_secret_never_leaks_in_line_events() {
        let (mut writer, reader) = tokio::io::duplex(32);
        let write_task = tokio::spawn(async move {
            writer.write_all(b"line1\nline2\n").await.unwrap();
        });
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = events.clone();
        let event_sink: EventSink = Arc::new(move |event| {
            collected.lock().unwrap().push(event);
        });
        let output = read_output(
            reader,
            "stdout",
            event_sink,
            vec!["line1\nline2".to_owned()],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        write_task.await.unwrap();
        assert_eq!(output, "line1\nline2\n");
        let events = events.lock().unwrap();
        assert!(!events.iter().any(|event| event.text.contains("line1")));
        assert!(!events.iter().any(|event| event.text.contains("line2")));
    }
}
