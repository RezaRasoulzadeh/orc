//! Worker abstraction for executing tasks.
//! Keeps provider-specific logic behind this interface so tests can inject fake workers.

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct CancellationControl(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancellationControl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

use crate::backend;
use crate::registry::ReasoningEffort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerOutcome {
    /// Worker completed successfully
    Success,
    /// Worker failed with an error message
    Failure(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenUsage {
    pub total_tokens: i64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerExecution {
    pub outcome: WorkerOutcome,
    pub output: Option<String>,
    pub token_usage: Option<TokenUsage>,
}

pub const DEFAULT_WORKER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_VALIDATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub fn configured_timeout(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(default)
}

pub fn run_command_with_timeout(command: Command, timeout: Duration) -> Result<Output, String> {
    run_command_with_timeout_progress_and_cancel(command, timeout, None, |_| {})
}

pub fn run_command_with_timeout_progress(
    command: Command,
    timeout: Duration,
    progress: impl Fn(&str),
) -> Result<Output, String> {
    run_command_with_timeout_progress_and_stdin_cancel(command, timeout, None, None, progress)
}

pub fn run_command_with_timeout_progress_and_stdin(
    command: Command,
    timeout: Duration,
    stdin: Option<&[u8]>,
    progress: impl Fn(&str),
) -> Result<Output, String> {
    run_command_with_timeout_progress_and_stdin_cancel(command, timeout, stdin, None, progress)
}

pub fn run_command_with_timeout_progress_and_cancel(
    command: Command,
    timeout: Duration,
    cancellation: Option<&CancellationControl>,
    progress: impl Fn(&str),
) -> Result<Output, String> {
    run_command_with_timeout_progress_and_stdin_cancel(
        command,
        timeout,
        None,
        cancellation,
        progress,
    )
}

pub fn run_command_with_timeout_progress_and_stdin_cancel(
    mut command: Command,
    timeout: Duration,
    stdin: Option<&[u8]>,
    cancellation: Option<&CancellationControl>,
    progress: impl Fn(&str),
) -> Result<Output, String> {
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn command: {error}"))?;
    let _stdin_writer = stdin.and_then(|input| {
        child.stdin.take().map(|mut pipe| {
            let input = input.to_owned();
            std::thread::spawn(move || {
                let _ = pipe.write_all(&input);
            })
        })
    });
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("spawned command did not provide stdout pipe".to_string());
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("spawned command did not provide stderr pipe".to_string());
    };
    let (sender, receiver) = mpsc::channel();
    let _stdout_reader = spawn_reader(stdout, 0, sender.clone());
    let _stderr_reader = spawn_reader(stderr, 1, sender);
    let mut stdout_output = Vec::new();
    let mut stderr_output = Vec::new();
    let mut stdout_pending = Vec::new();
    let mut stderr_pending = Vec::new();
    let mut readers = 2;
    let mut receive = |stream: usize, bytes: Vec<u8>| {
        let (output, pending) = if stream == 0 {
            (&mut stdout_output, &mut stdout_pending)
        } else {
            (&mut stderr_output, &mut stderr_pending)
        };
        output.extend_from_slice(&bytes);
        pending.extend_from_slice(&bytes);
        while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
            let line = String::from_utf8_lossy(&pending[..index]).trim().to_owned();
            pending.drain(..=index);
            if !line.is_empty() {
                progress(&line);
            }
        }
    };
    let started = Instant::now();
    loop {
        if cancellation.is_some_and(CancellationControl::is_cancelled) {
            #[cfg(unix)]
            unsafe {
                kill(-(child.id() as i32), SIGTERM);
            }
            #[cfg(not(unix))]
            {
                let _ = child.kill();
            }
            let _ = child.wait();
            return Err("execution cancelled at process boundary".into());
        }
        while let Ok((stream, bytes)) = receiver.try_recv() {
            let done = bytes.is_empty();
            receive(stream, bytes);
            if done {
                readers -= 1;
            }
        }
        if child
            .try_wait()
            .map_err(|error| format!("failed waiting for command: {error}"))?
            .is_some()
        {
            let status = child
                .wait()
                .map_err(|error| format!("failed reaping command: {error}"))?;
            while readers > 0 {
                if let Ok((stream, bytes)) = receiver.recv() {
                    let done = bytes.is_empty();
                    receive(stream, bytes);
                    if done {
                        readers -= 1;
                    }
                }
            }
            flush_progress(&mut stdout_pending, &progress);
            flush_progress(&mut stderr_pending, &progress);
            return Ok(Output {
                status,
                stdout: stdout_output,
                stderr: stderr_output,
            });
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            {
                unsafe {
                    kill(-(child.id() as i32), SIGTERM);
                }
                let grace_deadline = Instant::now() + Duration::from_secs(1);
                while Instant::now() < grace_deadline {
                    if child
                        .try_wait()
                        .map_err(|error| format!("failed waiting after timeout: {error}"))?
                        .is_some()
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                // The direct child may have exited while a descendant keeps the
                // pipes open. Kill the owned group regardless, then reap the child.
                unsafe {
                    kill(-(child.id() as i32), SIGKILL);
                }
            }
            #[cfg(not(unix))]
            child
                .kill()
                .map_err(|error| format!("failed terminating timed-out command: {error}"))?;
            let _status = child
                .wait()
                .map_err(|error| format!("failed reaping timed-out command: {error}"))?;
            while readers > 0 {
                if let Ok((stream, bytes)) = receiver.recv() {
                    let done = bytes.is_empty();
                    receive(stream, bytes);
                    if done {
                        readers -= 1;
                    }
                }
            }
            flush_progress(&mut stdout_pending, &progress);
            flush_progress(&mut stderr_pending, &progress);
            return Err(format!(
                "external process timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        match receiver.recv_timeout(Duration::from_millis(25)) {
            Ok((stream, bytes)) => {
                let done = bytes.is_empty();
                receive(stream, bytes);
                if done {
                    readers -= 1;
                }
            }
            Err(RecvTimeoutError::Disconnected) => readers = 0,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    stream: usize,
    sender: mpsc::Sender<(usize, Vec<u8>)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send((stream, Vec::new()));
                    break;
                }
                Ok(size) => {
                    if sender.send((stream, buffer[..size].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn flush_progress(pending: &mut Vec<u8>, progress: &impl Fn(&str)) {
    let line = String::from_utf8_lossy(pending).trim().to_owned();
    if !line.is_empty() {
        progress(&line);
    }
    pending.clear();
}

#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> i32;
    fn kill(pid: i32, signal: i32) -> i32;
}

#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

/// Trait for task execution backends.
/// Implementations are responsible for executing a task and returning the outcome.
pub trait Worker: Send + Sync {
    /// Convert a provider event into an Orc activity message. Event formats
    /// stay inside the provider implementation.
    fn activity(&self, _event: &str) -> String {
        "provider activity".into()
    }

    /// Provider execution settings selected for this worker, when applicable.
    fn execution_configuration(&self) -> (Option<&str>, Option<ReasoningEffort>) {
        (None, None)
    }
    /// Provider environment required to execute this worker, if any.
    fn configured_environment(&self) -> Option<(&'static str, &Path)> {
        None
    }

    /// Execute a task with the given prompt and return the outcome.
    /// output may contain captured stdout/stderr or other useful diagnostic information.
    /// cwd is the working directory in which the task should execute.
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String>;

    fn execute_with_progress(
        &self,
        prompt: &str,
        cwd: &Path,
        _progress: &dyn Fn(&str),
    ) -> Result<(WorkerOutcome, Option<String>), String> {
        self.execute(prompt, cwd)
    }

    fn execute_with_progress_and_usage(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        let (outcome, output) = self.execute_with_progress(prompt, cwd, progress)?;
        Ok(WorkerExecution {
            outcome,
            output,
            token_usage: None,
        })
    }

    fn execute_with_progress_and_usage_cancellable(
        &self,
        _prompt: &str,
        _cwd: &Path,
        _progress: &dyn Fn(&str),
        _cancellation: &CancellationControl,
    ) -> Result<WorkerExecution, String> {
        Err("worker does not support cooperative cancellation".into())
    }

    fn execute_structured_with_progress_and_usage(
        &self,
        prompt: &str,
        cwd: &Path,
        _schema: &str,
        progress: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        self.execute_with_progress_and_usage(prompt, cwd, progress)
    }

    /// Execute exactly one persisted PREPARE step.  This is the execution seam
    /// for the provider-independent protocol; callers must invoke it in plan
    /// order and may not collapse a plan into one provider request.
    fn execute_planned_step(
        &self,
        step: &crate::worker_protocol::PlannedStep,
        context: &str,
        cwd: &Path,
        schema: &str,
        progress: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        let prompt = format!(
            "{context}\n\nWORKER PLAN STEP (execute only this step):\n{}",
            serde_json::to_string_pretty(step).map_err(|e| e.to_string())?
        );
        self.execute_structured_with_progress_and_usage(&prompt, cwd, schema, progress)
    }

    /// Cancellable counterpart that retains the persisted-step execution seam.
    /// Backends which support cooperative cancellation may override this; the
    /// default preserves correctness by still executing exactly one step.
    fn execute_planned_step_cancellable(
        &self,
        step: &crate::worker_protocol::PlannedStep,
        context: &str,
        cwd: &Path,
        schema: &str,
        progress: &dyn Fn(&str),
        _cancellation: &CancellationControl,
    ) -> Result<WorkerExecution, String> {
        self.execute_planned_step(step, context, cwd, schema, progress)
    }
}

/// Copilot CLI worker.
///
/// Copilot's documented programmatic interface is a plain-text prompt (`-p`)
/// that exits after completion. It has no provider-structured event format,
/// so the generic Worker protocol remains the only structured boundary Orc
/// relies on.
pub struct CopilotWorker {
    copilot_home: Option<PathBuf>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    executable: PathBuf,
}

impl CopilotWorker {
    pub fn with_execution(
        copilot_home: Option<PathBuf>,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        Self {
            copilot_home,
            model,
            reasoning_effort,
            executable: PathBuf::from("copilot"),
        }
    }

    pub fn with_executable(mut self, executable: PathBuf) -> Self {
        self.executable = executable;
        self
    }

    /// Build the flags documented for non-interactive Copilot CLI use.
    pub fn command_args(
        prompt: &str,
        model: Option<&str>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Vec<String> {
        let mut args = vec![
            "-p".into(),
            prompt.into(),
            "-s".into(),
            "--allow-all-tools".into(),
            "--no-ask-user".into(),
        ];
        if let Some(model) = model {
            args.extend(["--model".into(), model.into()]);
        }
        // Orc's `None` means provider default. Copilot's documented effort
        // values do not include a `none` setting, so do not pass one.
        if let Some(effort) = reasoning_effort.filter(|value| *value != ReasoningEffort::None) {
            args.extend(["--effort".into(), effort.as_str().into()]);
        }
        args
    }

    fn command(&self, prompt: &str, cwd: &Path) -> Command {
        let mut command = Command::new(&self.executable);
        command.args(Self::command_args(
            prompt,
            self.model.as_deref(),
            self.reasoning_effort,
        ));
        if let Some(copilot_home) = &self.copilot_home {
            command.env("COPILOT_HOME", copilot_home);
        }
        backend::configure_noninteractive(&mut command, cwd);
        command
    }
}

impl Worker for CopilotWorker {
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        self.execute_with_progress(prompt, cwd, &|_| {})
    }

    fn execute_with_progress(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
    ) -> Result<(WorkerOutcome, Option<String>), String> {
        match run_command_with_timeout_progress(
            self.command(prompt, cwd),
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
            progress,
        ) {
            Ok(status) => {
                if status.status.success() {
                    let output = String::from_utf8_lossy(&status.stdout).to_string();
                    Ok((
                        WorkerOutcome::Success,
                        (!output.is_empty()).then_some(output),
                    ))
                } else {
                    let code = status
                        .status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    Err(format!("Copilot CLI exited with non-zero status: {code}"))
                }
            }
            Err(e) => Err(format!(
                "failed to spawn 'copilot' executable; ensure it is installed and on PATH: {e}",
            )),
        }
    }

    fn execution_configuration(&self) -> (Option<&str>, Option<ReasoningEffort>) {
        (self.model.as_deref(), self.reasoning_effort)
    }

    fn configured_environment(&self) -> Option<(&'static str, &Path)> {
        self.copilot_home
            .as_deref()
            .map(|path| ("COPILOT_HOME", path))
    }

    fn execute_with_progress_and_usage_cancellable(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
        cancellation: &CancellationControl,
    ) -> Result<WorkerExecution, String> {
        let output = run_command_with_timeout_progress_and_cancel(
            self.command(prompt, cwd),
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
            Some(cancellation),
            progress,
        )?;
        if !output.status.success() {
            return Err(format!(
                "Copilot CLI exited with non-zero status: {}",
                output
                    .status
                    .code()
                    .map_or("unknown".into(), |code| code.to_string())
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(WorkerExecution {
            outcome: WorkerOutcome::Success,
            output: (!text.is_empty()).then_some(text),
            token_usage: None,
        })
    }
}

/// Codex CLI worker. Its required profile is isolated through CODEX_HOME.
pub struct CodexWorker {
    pub profile_path: PathBuf,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    sandbox: &'static str,
    executable: PathBuf,
}

impl CodexWorker {
    pub fn new(profile_path: PathBuf) -> Self {
        Self::with_execution(profile_path, None, None)
    }

    pub fn with_execution(
        profile_path: PathBuf,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        Self {
            profile_path,
            model,
            reasoning_effort,
            sandbox: "workspace-write",
            executable: PathBuf::from("codex"),
        }
    }

    pub fn with_read_only_execution(
        profile_path: PathBuf,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Self {
        Self {
            profile_path,
            model,
            reasoning_effort,
            sandbox: "read-only",
            executable: PathBuf::from("codex"),
        }
    }

    pub fn with_executable(mut self, executable: PathBuf) -> Self {
        self.executable = executable;
        self
    }

    pub fn command_args(prompt: &str) -> Vec<String> {
        Self::command_args_with_execution(prompt, None, None)
    }

    pub fn command_args_with_execution(
        prompt: &str,
        model: Option<&str>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Vec<String> {
        Self::command_args_with_sandbox(prompt, model, reasoning_effort, "workspace-write")
    }

    pub fn command_args_with_sandbox(
        _prompt: &str,
        model: Option<&str>,
        reasoning_effort: Option<ReasoningEffort>,
        sandbox: &str,
    ) -> Vec<String> {
        vec![
            "exec".into(),
            "--json".into(),
            "--sandbox".into(),
            sandbox.into(),
        ]
        .into_iter()
        .chain(
            model
                .into_iter()
                .flat_map(|model| ["--model".into(), model.into()]),
        )
        .chain(reasoning_effort.into_iter().flat_map(|effort| {
            [
                "--config".into(),
                format!("model_reasoning_effort=\"{}\"", effort.as_str()),
            ]
        }))
        .chain(std::iter::once("-".into()))
        .collect()
    }

    fn command_with_schema(
        &self,
        prompt: &str,
        cwd: &Path,
        schema_path: Option<&Path>,
    ) -> std::process::Command {
        let mut command = std::process::Command::new(&self.executable);
        let mut args = Self::command_args_with_sandbox(
            prompt,
            self.model.as_deref(),
            self.reasoning_effort,
            self.sandbox,
        );
        if let Some(path) = schema_path {
            let stdin_marker = args.pop().expect("Codex command includes stdin marker");
            args.push("--output-schema".into());
            args.push(path.to_string_lossy().into_owned());
            args.push(stdin_marker);
        }
        command.args(args);
        backend::apply_profile_environment(&mut command, &self.profile_path);
        backend::configure_noninteractive(&mut command, cwd);
        command
    }

    #[cfg(test)]
    fn command_args_for_test(&self) -> Vec<String> {
        Self::command_args_with_sandbox(
            "",
            self.model.as_deref(),
            self.reasoning_effort,
            self.sandbox,
        )
    }
}

impl Worker for CodexWorker {
    fn activity(&self, event: &str) -> String {
        codex_activity(event)
    }

    fn execution_configuration(&self) -> (Option<&str>, Option<ReasoningEffort>) {
        (self.model.as_deref(), self.reasoning_effort)
    }
    fn configured_environment(&self) -> Option<(&'static str, &Path)> {
        Some(("CODEX_HOME", &self.profile_path))
    }

    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        self.execute_with_progress(prompt, cwd, &|_| {})
    }

    fn execute_with_progress(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
    ) -> Result<(WorkerOutcome, Option<String>), String> {
        let execution = self.execute_with_progress_and_usage(prompt, cwd, progress)?;
        Ok((execution.outcome, execution.output))
    }

    fn execute_with_progress_and_usage(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        self.run(prompt, cwd, None, progress)
    }

    fn execute_with_progress_and_usage_cancellable(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
        cancellation: &CancellationControl,
    ) -> Result<WorkerExecution, String> {
        self.run_cancellable(prompt, cwd, None, progress, Some(cancellation))
    }

    fn execute_structured_with_progress_and_usage(
        &self,
        prompt: &str,
        cwd: &Path,
        schema: &str,
        progress: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        use std::io::Write;

        let mut path = std::env::temp_dir();
        static SCHEMA_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SCHEMA_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        path.push(format!(
            "orc-output-schema-{}-{sequence}.json",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("failed to create Codex output schema: {error}"))?;
        if let Err(error) = file.write_all(schema.as_bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("failed to write Codex output schema: {error}"));
        }
        drop(file);
        let result = self.run(prompt, cwd, Some(&path), progress);
        let cleanup = std::fs::remove_file(&path);
        match (result, cleanup) {
            (Ok(execution), Ok(())) => Ok(execution),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(format!("failed to remove Codex output schema: {error}")),
        }
    }
}

fn codex_activity(event: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(event) else {
        return "provider activity".into();
    };
    let event_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("activity");
    let item_type = value
        .pointer("/item/type")
        .and_then(serde_json::Value::as_str);
    match item_type {
        Some(item_type) => format!("provider {event_type}: {item_type}"),
        None => format!("provider {event_type}"),
    }
}

impl CodexWorker {
    fn run(
        &self,
        prompt: &str,
        cwd: &Path,
        schema_path: Option<&Path>,
        progress: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        self.run_cancellable(prompt, cwd, schema_path, progress, None)
    }

    fn run_cancellable(
        &self,
        prompt: &str,
        cwd: &Path,
        schema_path: Option<&Path>,
        progress: &dyn Fn(&str),
        cancellation: Option<&CancellationControl>,
    ) -> Result<WorkerExecution, String> {
        match run_command_with_timeout_progress_and_stdin_cancel(
            self.command_with_schema(prompt, cwd, schema_path),
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
            Some(prompt.as_bytes()),
            cancellation,
            progress,
        ) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let (final_output, token_usage) =
                    parse_codex_jsonl(&stdout, schema_path.is_some())?;
                let combined = combine_codex_output(final_output, &stderr, schema_path.is_some());
                Ok(WorkerExecution {
                    outcome: WorkerOutcome::Success,
                    output: combined,
                    token_usage,
                })
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(codex_failure_error(
                    &output.status,
                    &stderr,
                    schema_path.is_some(),
                ))
            }
            Err(error) if error.contains("No such file or directory") => Err(format!(
                "failed to spawn 'codex' executable; ensure it is installed and on PATH: {error}"
            )),
            Err(error) => Err(format!("failed to spawn 'codex' executable: {error}")),
        }
    }
}

fn parse_codex_jsonl(
    output: &str,
    structured: bool,
) -> Result<(Option<String>, Option<TokenUsage>), String> {
    let mut messages = Vec::new();
    let mut final_message = None;
    let mut usage = None;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let event: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid JSON event from Codex: {error}"))?;
        if event.get("type").and_then(serde_json::Value::as_str) == Some("item.completed")
            && event
                .pointer("/item/type")
                .and_then(serde_json::Value::as_str)
                == Some("agent_message")
            && let Some(text) = event
                .pointer("/item/text")
                .and_then(serde_json::Value::as_str)
        {
            if structured {
                final_message = Some(text.to_owned());
            } else {
                messages.push(text.to_owned());
            }
        }
        if event.get("type").and_then(serde_json::Value::as_str) == Some("turn.completed")
            && let Some(value) = event.get("usage")
        {
            let input_tokens = value
                .get("input_tokens")
                .and_then(serde_json::Value::as_i64);
            let output_tokens = value
                .get("output_tokens")
                .and_then(serde_json::Value::as_i64);
            let total_tokens = value
                .get("total_tokens")
                .and_then(serde_json::Value::as_i64)
                .or_else(|| {
                    input_tokens
                        .zip(output_tokens)
                        .map(|(input, output)| input + output)
                });
            if let Some(total_tokens) = total_tokens {
                usage = Some(TokenUsage {
                    total_tokens,
                    input_tokens,
                    output_tokens,
                });
            }
        }
    }
    let output = if structured {
        final_message
    } else {
        (!messages.is_empty()).then(|| messages.join("\n"))
    };
    Ok((output, usage))
}

fn combine_codex_output(
    final_output: Option<String>,
    stderr: &str,
    structured: bool,
) -> Option<String> {
    if structured {
        return final_output;
    }
    match (final_output, stderr.is_empty()) {
        (Some(output), true) => Some(output),
        (Some(output), false) => Some(format!("{output}\n{stderr}")),
        (None, false) => Some(stderr.to_owned()),
        (None, true) => None,
    }
}

fn codex_failure_error(
    status: &std::process::ExitStatus,
    stderr: &str,
    structured: bool,
) -> String {
    let code = status
        .code()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".into());
    let stderr = stderr.trim();
    if structured && !stderr.is_empty() {
        format!("Codex exited with non-zero status: {code}: {stderr}")
    } else {
        format!("Codex exited with non-zero status: {code}")
    }
}

/// Antigravity CLI worker. Runs `agy` in headless print mode with JSON output
/// and accept-edits so file operations proceed without an interactive prompt,
/// while leaving shell-command permissions on the CLI's default (safe) policy.
pub struct AntigravityWorker;

impl AntigravityWorker {
    pub fn command_args(prompt: &str) -> Vec<String> {
        vec![
            "-p".into(),
            prompt.into(),
            "--output-format".into(),
            "json".into(),
            "--mode".into(),
            "accept-edits".into(),
            "--sandbox".into(),
        ]
    }
}

impl Worker for AntigravityWorker {
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        self.execute_with_progress(prompt, cwd, &|_| {})
    }

    fn execute_with_progress(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
    ) -> Result<(WorkerOutcome, Option<String>), String> {
        let mut command = std::process::Command::new("agy");
        command.args(Self::command_args(prompt));
        backend::configure_noninteractive(&mut command, cwd);
        match run_command_with_timeout_progress(
            command,
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
            progress,
        ) {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let combined = if stderr.is_empty() {
                    stdout
                } else if stdout.is_empty() {
                    stderr
                } else {
                    format!("{stdout}\n{stderr}")
                };
                Ok((
                    WorkerOutcome::Success,
                    (!combined.is_empty()).then_some(combined),
                ))
            }
            Ok(output) => {
                let code = output
                    .status
                    .code()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".into());
                Err(format!("Antigravity exited with non-zero status: {code}"))
            }
            Err(error) => Err(format!(
                "failed to spawn 'agy' executable; ensure it is installed and on PATH: {error}"
            )),
        }
    }

    fn execute_with_progress_and_usage_cancellable(
        &self,
        prompt: &str,
        cwd: &Path,
        progress: &dyn Fn(&str),
        cancellation: &CancellationControl,
    ) -> Result<WorkerExecution, String> {
        let mut command = std::process::Command::new("agy");
        command.args(Self::command_args(prompt));
        backend::configure_noninteractive(&mut command, cwd);
        let output = run_command_with_timeout_progress_and_cancel(
            command,
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
            Some(cancellation),
            progress,
        )?;
        if !output.status.success() {
            return Err(format!(
                "Antigravity exited with non-zero status: {}",
                output
                    .status
                    .code()
                    .map_or("unknown".into(), |code| code.to_string())
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let text = match (stdout.is_empty(), stderr.is_empty()) {
            (true, true) => String::new(),
            (false, true) => stdout.into_owned(),
            (true, false) => stderr.into_owned(),
            (false, false) => format!("{stdout}\n{stderr}"),
        };
        Ok(WorkerExecution {
            outcome: WorkerOutcome::Success,
            output: (!text.is_empty()).then_some(text),
            token_usage: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_execution_uses_read_only_sandbox() {
        let args =
            CodexWorker::with_read_only_execution(PathBuf::from("/profiles/lead"), None, None)
                .command_args_for_test();
        assert_eq!(args, vec!["exec", "--json", "--sandbox", "read-only", "-"]);
    }

    #[test]
    fn codex_json_events_preserve_final_message_and_reported_usage() {
        let events = r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}
{"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":20,"output_tokens":30}}"#;
        let (output, usage) = parse_codex_jsonl(events, false).unwrap();
        assert_eq!(output.as_deref(), Some("done"));
        assert_eq!(
            usage,
            Some(TokenUsage {
                total_tokens: 150,
                input_tokens: Some(120),
                output_tokens: Some(30),
            })
        );
    }

    #[test]
    fn codex_json_events_leave_unreported_usage_unavailable() {
        let (output, usage) = parse_codex_jsonl(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
            false,
        )
        .unwrap();
        assert_eq!(output.as_deref(), Some("done"));
        assert_eq!(usage, None);
    }

    #[test]
    fn codex_structured_json_events_use_only_the_final_message() {
        let events = r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"verdict\":\"reviewing\"}"}}
{"type":"item.completed","item":{"type":"agent_message","text":"{\"verdict\":\"revise\"}"}}"#;
        let (output, usage) = parse_codex_jsonl(events, true).unwrap();
        assert_eq!(output.as_deref(), Some(r#"{"verdict":"revise"}"#));
        assert_eq!(usage, None);
    }

    #[test]
    fn codex_structured_success_ignores_stderr_in_result() {
        let output = combine_codex_output(
            Some(r#"{"verdict":"pass"}"#.to_owned()),
            "Reading additional input from stdin...",
            true,
        );
        assert_eq!(output.as_deref(), Some(r#"{"verdict":"pass"}"#));
    }

    #[cfg(unix)]
    #[test]
    fn codex_structured_failure_preserves_stderr_diagnostics() {
        let output = run_command_with_timeout(
            shell("printf 'schema rejected\\n' >&2; exit 7"),
            Duration::from_secs(5),
        )
        .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error = codex_failure_error(&output.status, &stderr, true);
        assert_eq!(
            error,
            "Codex exited with non-zero status: 7: schema rejected"
        );
    }

    #[test]
    fn codex_commands_receive_the_registered_profile_environment() {
        let main = CodexWorker::new(PathBuf::from("/profiles/main"));
        let third = CodexWorker::new(PathBuf::from("/profiles/third"));
        let profile = |worker: &CodexWorker| {
            worker
                .command_with_schema("inspect", Path::new("."), None)
                .get_envs()
                .find(|(key, _)| *key == "CODEX_HOME")
                .and_then(|(_, value)| value)
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        };

        assert_eq!(profile(&main).as_deref(), Some("/profiles/main"));
        assert_eq!(profile(&third).as_deref(), Some("/profiles/third"));
    }

    #[test]
    fn codex_structured_command_uses_native_output_schema() {
        let worker = CodexWorker::new(PathBuf::from("/profiles/main"));
        let command = worker.command_with_schema(
            "inspect",
            Path::new("."),
            Some(Path::new("/tmp/schema.json")),
        );
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(args.windows(2).any(|values| {
            values
                == [
                    "--output-schema".to_string(),
                    "/tmp/schema.json".to_string(),
                ]
        }));
        assert_eq!(args.last().map(String::as_str), Some("-"));
    }

    #[cfg(unix)]
    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        command
    }

    #[cfg(unix)]
    #[test]
    fn drains_large_stdout_and_stderr_without_losing_output() {
        let output = run_command_with_timeout(
            shell("printf '%*s' 262144 '' | tr ' ' o; printf '%*s' 262144 '' | tr ' ' e >&2"),
            Duration::from_secs(5),
        )
        .expect("command should complete");
        assert_eq!(output.stdout.len(), 262144);
        assert_eq!(output.stderr.len(), 262144);
        assert!(output.stdout.iter().all(|byte| *byte == b'o'));
        assert!(output.stderr.iter().all(|byte| *byte == b'e'));
    }

    #[cfg(unix)]
    #[test]
    fn streams_complete_large_input_through_stdin() {
        let input = "prompt-".repeat(32 * 1024);
        let output = run_command_with_timeout_progress_and_stdin(
            shell("cat"),
            Duration::from_secs(5),
            Some(input.as_bytes()),
            |_| {},
        )
        .expect("command should complete");
        assert_eq!(output.stdout, input.as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn reports_both_streams_before_completion_and_preserves_output() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            run_command_with_timeout_progress(
                shell("printf 'out\\n'; printf 'err\\n' >&2; sleep 1"),
                Duration::from_secs(5),
                |line| sender.send(line.to_owned()).unwrap(),
            )
        });
        let first = receiver
            .recv_timeout(Duration::from_millis(500))
            .expect("output should arrive while the process is running");
        assert!(first == "out" || first == "err");
        let second = receiver.recv_timeout(Duration::from_millis(500)).unwrap();
        assert!(first != second);
        let output = handle.join().unwrap().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_the_owned_process_group() {
        let output = std::env::temp_dir().join(format!("orc-timeout-{}", std::process::id()));
        let script = format!("sleep 30 & echo $! > {}; wait", output.display());
        let error = run_command_with_timeout(shell(&script), Duration::from_millis(50))
            .expect_err("command should time out");
        assert!(error.contains("timed out"));
        let descendant = std::fs::read_to_string(&output).expect("child pid should be written");
        let _ = std::fs::remove_file(&output);
        let status = Command::new("kill")
            .args(["-0", descendant.trim()])
            .status();
        assert!(!status.expect("kill should run").success());
    }
}

pub mod test_helpers {
    use super::*;

    /// Fake worker for testing
    pub struct FakeWorker {
        pub outcome: WorkerOutcome,
        pub output: Option<String>,
    }

    impl FakeWorker {
        pub fn new_success(output: Option<String>) -> Self {
            Self {
                outcome: WorkerOutcome::Success,
                output,
            }
        }

        pub fn new_failure(error: String) -> Self {
            Self {
                outcome: WorkerOutcome::Failure(error),
                output: None,
            }
        }
    }

    impl Worker for FakeWorker {
        fn execute(
            &self,
            _prompt: &str,
            cwd: &Path,
        ) -> Result<(WorkerOutcome, Option<String>), String> {
            if matches!(self.outcome, WorkerOutcome::Success) {
                std::fs::write(cwd.join("fake-worker-change.txt"), "fake worker change\n")
                    .map_err(|error| error.to_string())?;
            }
            Ok((self.outcome.clone(), self.output.clone()))
        }
    }

    /// Worker that always fails at spawn time (for testing spawn failure handling)
    pub struct FailingSpawnWorker;

    impl Worker for FailingSpawnWorker {
        fn execute(
            &self,
            _prompt: &str,
            _cwd: &Path,
        ) -> Result<(WorkerOutcome, Option<String>), String> {
            Err("simulated spawn failure".to_string())
        }
    }
}
