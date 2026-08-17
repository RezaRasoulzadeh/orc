//! Worker abstraction for executing tasks.
//! Keeps provider-specific logic behind this interface so tests can inject fake workers.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerOutcome {
    /// Worker completed successfully
    Success,
    /// Worker failed with an error message
    Failure(String),
}

/// Trait for task execution backends.
/// Implementations are responsible for executing a task and returning the outcome.
pub trait Worker: Send + Sync {
    /// Execute a task with the given prompt and return the outcome.
    /// output may contain captured stdout/stderr or other useful diagnostic information.
    /// cwd is the working directory in which the task should execute.
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String>;
}

/// Copilot worker implementation
pub struct CopilotWorker;

impl Worker for CopilotWorker {
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        use std::process::{Command, Stdio};

        let mut cmd = Command::new("copilot");
        cmd.arg("-p").arg(prompt).arg("--allow-all-tools");
        cmd.current_dir(cwd);
        cmd.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit());

        match cmd.status() {
            Ok(status) => {
                if status.success() {
                    Ok((WorkerOutcome::Success, None))
                } else {
                    let code = status
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
            _cwd: &Path,
        ) -> Result<(WorkerOutcome, Option<String>), String> {
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
