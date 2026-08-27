use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::registry::AgentDefinition;
use crate::registry::ReasoningEffort;
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
    pub fn build_lead(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String> {
        if agent.backend != "codex" {
            return Err(format!(
                "Lead backend '{}' has no read-only execution boundary",
                agent.backend
            ));
        }
        let profile_path = agent.profile_path.as_deref().ok_or_else(|| {
            format!(
                "Codex agent '{}' requires a configured profile path",
                agent.id
            )
        })?;
        Ok(Box::new(CodexWorker::with_read_only_execution(
            PathBuf::from(profile_path),
            model.or_else(|| agent.model.clone()),
            reasoning_effort.or(agent.reasoning_effort),
        )))
    }

    /// Build the Planner with the same enforced read-only boundary as Lead.
    pub fn build_planner(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String> {
        Self::build_planner_with_executable(agent, model, reasoning_effort, None)
    }

    pub fn build_planner_with_executable(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
        executable: Option<PathBuf>,
    ) -> Result<Box<dyn Worker>, String> {
        if agent.backend != "codex" {
            return Err(format!(
                "Planner backend '{}' has no read-only execution boundary",
                agent.backend
            ));
        }
        let profile_path = agent.profile_path.as_deref().ok_or_else(|| {
            format!(
                "Codex Planner agent '{}' requires a configured profile path",
                agent.id
            )
        })?;
        let worker = CodexWorker::with_read_only_execution(
            PathBuf::from(profile_path),
            model.or_else(|| agent.model.clone()),
            reasoning_effort.or(agent.reasoning_effort),
        );
        let worker = match executable {
            Some(path) => worker.with_executable(path),
            None => worker,
        };
        Ok(Box::new(worker))
    }

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
                Ok(Box::new(CodexWorker::with_execution(
                    PathBuf::from(profile_path),
                    agent.model.clone(),
                    agent.reasoning_effort,
                )))
            }
            "antigravity" => Ok(Box::new(AntigravityWorker)),
            backend => Err(format!(
                "unsupported agent backend '{}'; supported backends: copilot, codex, antigravity",
                backend
            )),
        }
    }

    pub fn build_with_codex_overrides(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String> {
        match agent.backend.as_str() {
            "codex" => {
                let profile_path = agent.profile_path.as_deref().ok_or_else(|| {
                    format!(
                        "Codex agent '{}' requires a configured profile path; run `orc agent profile {} <path>`",
                        agent.id, agent.id
                    )
                })?;
                Ok(Box::new(CodexWorker::with_execution(
                    PathBuf::from(profile_path),
                    model.or_else(|| agent.model.clone()),
                    reasoning_effort.or(agent.reasoning_effort),
                )))
            }
            _ if model.is_some() || reasoning_effort.is_some() => Err(format!(
                "backend '{}' does not support Codex model or reasoning-effort overrides",
                agent.backend
            )),
            _ => Self::build(agent),
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

    fn agent(backend: &str) -> AgentDefinition {
        serde_json::from_value(serde_json::json!({
            "id": "planner", "backend": backend, "execution_mode": "automated",
            "display_name": "Planner", "enabled": true, "priority": 0,
            "capabilities": [], "status": "available", "unavailable_reason": null,
            "profile_path": "/profiles/planner", "model": null, "reasoning_effort": null,
            "config_metadata": null, "quota_remaining_percent": null,
            "quota_reset_at": null, "quota_checked_at": null, "quota_source": null,
            "quota_limits": null, "actions": ["Plan"]
        }))
        .unwrap()
    }

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

    #[test]
    fn planner_rejects_backends_without_a_read_only_boundary() {
        let error = match WorkerFactory::build_planner(&agent("copilot"), None, None) {
            Ok(_) => panic!("unsupported Planner backend was accepted"),
            Err(error) => error,
        };
        assert!(error.contains("no read-only execution boundary"));
    }
}
