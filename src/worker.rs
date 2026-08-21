//! Worker abstraction for executing tasks.
//! Keeps provider-specific logic behind this interface so tests can inject fake workers.

use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::backend;
use crate::registry::ReasoningEffort;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerOutcome {
    /// Worker completed successfully
    Success,
    /// Worker failed with an error message
    Failure(String),
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
    run_command_with_timeout_progress(command, timeout, |_| {})
}

pub fn run_command_with_timeout_progress(
    mut command: Command,
    timeout: Duration,
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
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to spawn command: {error}"))?;
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
}

/// Copilot worker implementation
pub struct CopilotWorker;

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
        use std::process::Command;

        let mut cmd = Command::new("copilot");
        cmd.arg("-p").arg(prompt).arg("--allow-all-tools");
        backend::configure_noninteractive(&mut cmd, cwd);

        match run_command_with_timeout_progress(
            cmd,
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
                    Err(format!("Copilot exited with non-zero status: {}", code))
                }
            }
            Err(e) => Err(format!(
                "failed to spawn 'copilot' executable; ensure it is installed and on PATH: {}",
                e
            )),
        }
    }
}

/// Codex CLI worker. Its required profile is isolated through CODEX_HOME.
pub struct CodexWorker {
    pub profile_path: PathBuf,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
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
        }
    }

    pub fn command_args(prompt: &str) -> Vec<String> {
        Self::command_args_with_execution(prompt, None, None)
    }

    pub fn command_args_with_execution(
        prompt: &str,
        model: Option<&str>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Vec<String> {
        vec!["exec".into(), "--sandbox".into(), "workspace-write".into()]
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
            .chain(std::iter::once(prompt.into()))
            .collect()
    }

    fn command(&self, prompt: &str, cwd: &Path) -> std::process::Command {
        let mut command = std::process::Command::new("codex");
        command.args(Self::command_args_with_execution(
            prompt,
            self.model.as_deref(),
            self.reasoning_effort,
        ));
        backend::apply_profile_environment(&mut command, &self.profile_path);
        backend::configure_noninteractive(&mut command, cwd);
        command
    }
}

impl Worker for CodexWorker {
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
        match run_command_with_timeout_progress(
            self.command(prompt, cwd),
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
                Err(format!("Codex exited with non-zero status: {code}"))
            }
            Err(error) => Err(format!(
                "failed to spawn 'codex' executable; ensure it is installed and on PATH: {error}"
            )),
        }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_commands_receive_the_registered_profile_environment() {
        let main = CodexWorker::new(PathBuf::from("/profiles/main"));
        let third = CodexWorker::new(PathBuf::from("/profiles/third"));
        let profile = |worker: &CodexWorker| {
            worker
                .command("inspect", Path::new("."))
                .get_envs()
                .find(|(key, _)| *key == "CODEX_HOME")
                .and_then(|(_, value)| value)
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        };

        assert_eq!(profile(&main).as_deref(), Some("/profiles/main"));
        assert_eq!(profile(&third).as_deref(), Some("/profiles/third"));
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
