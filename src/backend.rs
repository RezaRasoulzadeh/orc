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
            if !runner.executable_exists("codex") {
                return Err("provider CLI 'codex' not found".into());
            }
            let profile = agent.profile_path.as_deref().map(Path::new);
            if let Some(path) = profile
                && !path.is_dir()
            {
                return Err(format!("profile path does not exist: {}", path.display()));
            }
            runner.run(
                "codex",
                &["login", "status"],
                cwd,
                profile.map(|path| ("CODEX_HOME", path)),
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
            "codex" => Ok(Box::new(CodexWorker::new(
                agent.profile_path.as_deref().map(PathBuf::from),
            ))),
            "antigravity" => Ok(Box::new(AntigravityWorker)),
            backend => Err(format!(
                "unsupported agent backend '{}'; supported backends: copilot, codex, antigravity",
                backend
            )),
        }
    }
}

pub(crate) fn apply_profile_environment(command: &mut Command, profile_path: Option<&Path>) {
    if let Some(profile_path) = profile_path {
        // Credentials remain managed by the Codex CLI and are never copied into Orc's database.
        command.env("CODEX_HOME", profile_path);
    }
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
        apply_profile_environment(&mut command, Some(Path::new("/profiles/main")));
        let value = command
            .get_envs()
            .find(|(key, _)| *key == "CODEX_HOME")
            .and_then(|(_, value)| value)
            .and_then(|value| value.to_str());
        assert_eq!(value, Some("/profiles/main"));
    }
}
