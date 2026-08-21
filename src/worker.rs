//! Worker abstraction for executing tasks.
//! Keeps provider-specific logic behind this interface so tests can inject fake workers.

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
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

pub fn run_command_with_timeout(mut command: Command, timeout: Duration) -> Result<Output, String> {
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            return child.wait_with_output().map_err(|error| error.to_string());
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            {
                let _ = Command::new("kill")
                    .args(["-TERM", &format!("-{}", child.id())])
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "external process timed out after {} seconds",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn setsid() -> i32;
}

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
}

/// Copilot worker implementation
pub struct CopilotWorker;

impl Worker for CopilotWorker {
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        use std::process::Command;

        let mut cmd = Command::new("copilot");
        cmd.arg("-p").arg(prompt).arg("--allow-all-tools");
        backend::configure_noninteractive(&mut cmd, cwd);

        match run_command_with_timeout(
            cmd,
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
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
        match run_command_with_timeout(
            self.command(prompt, cwd),
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
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
        let mut command = std::process::Command::new("agy");
        command.args(Self::command_args(prompt));
        backend::configure_noninteractive(&mut command, cwd);
        match run_command_with_timeout(
            command,
            configured_timeout("ORC_WORKER_TIMEOUT_SECS", DEFAULT_WORKER_TIMEOUT),
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
