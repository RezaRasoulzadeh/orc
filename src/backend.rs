use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::registry::AgentDefinition;
use crate::worker::{AntigravityWorker, CodexWorker, CopilotWorker, Worker};

pub trait HealthCommandRunner {
    fn executable_exists(&self, executable: &str) -> bool;
    fn run(
        &self,
        executable: &str,
        args: &[&str],
        cwd: &Path,
        environment: Option<(&str, &Path)>,
    ) -> Result<(), String>;
}

pub fn check_health(
    agent: &AgentDefinition,
    cwd: &Path,
    runner: &dyn HealthCommandRunner,
) -> Result<(), String> {
    if agent.execution_mode == crate::registry::MANUAL {
        return Ok(());
    }
    match agent.backend.as_str() {
        "codex" => {
            let profile = agent.profile_path.as_deref().map(Path::new).ok_or_else(|| {
                format!(
                    "Codex agent '{}' requires a configured profile path; run `orc agent profile {} <path>`",
                    agent.id, agent.id
                )
            })?;
            if !runner.executable_exists("codex") {
                return Err("provider CLI 'codex' not found".into());
            }
            if !profile.is_dir() {
                return Err(format!(
                    "profile path does not exist: {}",
                    profile.display()
                ));
            }
            runner.run(
                "codex",
                &["login", "status"],
                cwd,
                Some(("CODEX_HOME", profile)),
            )
        }
        "copilot" => {
            if !runner.executable_exists("copilot") {
                return Err("provider CLI 'copilot' not found".into());
            }
            runner.run("copilot", &["--version"], cwd, None)
        }
        "antigravity" => {
            if !runner.executable_exists("agy") {
                return Err("provider CLI 'agy' not found".into());
            }
            runner.run("agy", &["--version"], cwd, None)
        }
        backend => Err(format!("unsupported backend '{backend}'")),
    }
}

pub struct WorkerFactory;

impl WorkerFactory {
    pub fn build(agent: &AgentDefinition) -> Result<Box<dyn Worker>, String> {
        match agent.backend.as_str() {
            "copilot" => Ok(Box::new(CopilotWorker)),
            "codex" => {
                let profile_path = agent.profile_path.as_deref().ok_or_else(|| {
                    format!(
                        "Codex agent '{}' requires a configured profile path; run `orc agent profile {} <path>`",
                        agent.id, agent.id
                    )
                })?;
                Ok(Box::new(CodexWorker::new(PathBuf::from(profile_path))))
            }
            "antigravity" => Ok(Box::new(AntigravityWorker)),
            backend => Err(format!(
                "unsupported agent backend '{}'; supported backends: copilot, codex, antigravity",
                backend
            )),
        }
    }
}

pub(crate) fn apply_profile_environment(command: &mut Command, profile_path: &Path) {
    // Credentials remain managed by the Codex CLI and are never copied into Orc's database.
    command.env("CODEX_HOME", profile_path);
}

pub(crate) fn configure_noninteractive(command: &mut Command, cwd: &Path) {
    command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_environment_is_scoped_to_the_codex_command() {
        let mut command = Command::new("codex");
        apply_profile_environment(&mut command, Path::new("/profiles/main"));
        let value = command
            .get_envs()
            .find(|(key, _)| *key == "CODEX_HOME")
            .and_then(|(_, value)| value)
            .and_then(|value| value.to_str());
        assert_eq!(value, Some("/profiles/main"));
    }
}
