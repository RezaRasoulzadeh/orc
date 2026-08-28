use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::backend::ProviderInspection;
use crate::registry::{
    self, Agent, AgentAction, AgentCapability, AgentDefinition, AgentExecutionMode,
    AgentProviderConfiguration, OperatorPermission, ReasoningEffort,
};
use crate::storage::{AgentAuthorization, Database};

/// Versioned, portable representation used by `agent export`, `agent import`,
/// and `agent update`. Credentials are never part of this document.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigurationDocument {
    pub configuration_version: u16,
    pub agent: Agent,
    pub permissions: Vec<OperatorPermission>,
    pub authentication: AgentAuthentication,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentAuthentication {
    pub verified: bool,
    pub method: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentOnboardingRequest {
    pub id: String,
    pub backend: String,
    pub execution_mode: AgentExecutionMode,
    pub display_name: String,
    pub profile_path: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub priority: i64,
    pub roles: Vec<AgentAction>,
    pub permissions: Vec<OperatorPermission>,
    /// Manual providers cannot be inspected by Orc; these are the provider
    /// capabilities declared by the operator for that external worker.
    pub declared_capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentOnboardingPreview {
    pub agent: AgentDefinition,
    pub provider_capabilities: Vec<AgentCapability>,
    pub permissions: Vec<OperatorPermission>,
    pub authentication: AgentAuthentication,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentOnboardingResult {
    pub preview: AgentOnboardingPreview,
    pub persisted: bool,
}

impl AgentOnboardingRequest {
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("agent id must not be empty")
        }
        if self.display_name.trim().is_empty() {
            bail!("agent display name must not be empty")
        }
        registry::validate_backend(&self.backend)?;
        if self.execution_mode == AgentExecutionMode::Automated
            && !matches!(self.backend.as_str(), "codex" | "copilot" | "antigravity")
        {
            bail!("backend '{}' requires manual execution mode", self.backend)
        }
        if (self.model.is_some() || self.reasoning_effort.is_some())
            && (self.backend != "codex" || self.execution_mode != AgentExecutionMode::Automated)
        {
            bail!("only automated Codex agents support model and reasoning-effort configuration")
        }
        if self.roles.is_empty() {
            bail!("at least one Orc role must be assigned")
        }
        Ok(())
    }

    fn definition(&self, inspection: &ProviderInspection) -> AgentDefinition {
        AgentDefinition {
            id: self.id.clone(),
            backend: self.backend.clone(),
            execution_mode: self.execution_mode.as_str().to_owned(),
            display_name: self.display_name.clone(),
            enabled: true,
            priority: self.priority,
            capabilities: inspection
                .capabilities
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect(),
            status: registry::AVAILABLE.to_owned(),
            unavailable_reason: None,
            profile_path: self.profile_path.clone(),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort,
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: self.roles.clone(),
        }
    }
}

pub trait ProviderOnboarding: Send + Sync {
    fn inspect(
        &self,
        provider: &AgentProviderConfiguration,
        mode: AgentExecutionMode,
        declared_capabilities: &[String],
    ) -> Result<ProviderInspection, String>;
}

/// Provider inspection performed without making an AI request. Provider
/// credentials remain in the provider CLI/profile and are only checked.
pub struct SystemProviderOnboarding {
    cwd: PathBuf,
    command_runner: Box<dyn ProviderCommandRunner>,
}

impl SystemProviderOnboarding {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            command_runner: Box::new(SystemProviderCommandRunner),
        }
    }

    #[cfg(test)]
    fn with_command_runner(
        cwd: impl Into<PathBuf>,
        command_runner: Box<dyn ProviderCommandRunner>,
    ) -> Self {
        Self {
            cwd: cwd.into(),
            command_runner,
        }
    }
}

struct ProviderCommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

trait ProviderCommandRunner: Send + Sync {
    fn run(
        &self,
        executable: &str,
        args: &[&str],
        cwd: &Path,
        environment: Option<(&str, &Path)>,
    ) -> Result<ProviderCommandOutput, String>;
}

struct SystemProviderCommandRunner;

impl ProviderCommandRunner for SystemProviderCommandRunner {
    fn run(
        &self,
        executable: &str,
        args: &[&str],
        cwd: &Path,
        environment: Option<(&str, &Path)>,
    ) -> Result<ProviderCommandOutput, String> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((key, value)) = environment {
            command.env(key, value);
        }
        let output = command
            .output()
            .map_err(|error| format!("failed to run {executable} authentication check: {error}"))?;
        Ok(ProviderCommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn authentication_check(
    backend: &str,
) -> Result<(&'static str, &'static [&'static str], &'static str), String> {
    match backend {
        "codex" => Ok(("codex", &["login", "status"], "codex_login_status")),
        // `/user` is Copilot's account inspection command. It checks the
        // logged-in account without issuing an AI prompt.
        "copilot" => Ok(("copilot", &["-p", "/user"], "copilot_user")),
        // Antigravity has no standalone auth-status subcommand. Listing
        // models is its non-interactive authenticated API operation and also
        // avoids sending an AI prompt during onboarding.
        "antigravity" => Ok(("agy", &["models"], "antigravity_models")),
        backend => Err(format!("unsupported agent backend '{backend}'")),
    }
}

impl ProviderOnboarding for SystemProviderOnboarding {
    fn inspect(
        &self,
        provider: &AgentProviderConfiguration,
        mode: AgentExecutionMode,
        declared_capabilities: &[String],
    ) -> Result<ProviderInspection, String> {
        if mode == AgentExecutionMode::Manual {
            return Ok(ProviderInspection {
                authenticated: true,
                authentication_method: "not_required".into(),
                authentication_detail: Some(
                    "manual provider authentication is operator-managed".into(),
                ),
                capabilities: declared_capabilities
                    .iter()
                    .map(|v| AgentCapability::parse(v))
                    .collect(),
            });
        }
        let (executable, args, authentication_method) = authentication_check(&provider.backend)?;
        if provider.backend == "codex" && provider.profile_path.is_none() {
            return Err("Codex onboarding requires a configured profile path".into());
        }
        if let Some(profile) = provider.profile_path.as_deref()
            && provider.backend == "codex"
            && !Path::new(profile).is_dir()
        {
            return Err(format!("profile path does not exist: {profile}"));
        }
        let output = self.command_runner.run(
            executable,
            args,
            &self.cwd,
            provider
                .profile_path
                .as_deref()
                .filter(|_| provider.backend == "codex")
                .map(|profile| ("CODEX_HOME", Path::new(profile))),
        )?;
        if !output.success {
            let detail = output.stderr.trim().to_owned();
            return Err(if detail.is_empty() {
                format!("{executable} authentication check failed")
            } else {
                detail
            });
        }
        Ok(ProviderInspection {
            authenticated: true,
            authentication_method: authentication_method.into(),
            authentication_detail: Some(output.stdout.trim().to_owned()),
            capabilities: discovered_capabilities(&provider.backend),
        })
    }
}

pub fn discovered_capabilities(backend: &str) -> Vec<AgentCapability> {
    match backend {
        "codex" | "copilot" | "antigravity" => vec![
            AgentCapability::Code,
            AgentCapability::RepositoryRead,
            AgentCapability::RepositoryWrite,
            AgentCapability::CommandExecution,
            AgentCapability::StructuredOutput,
        ],
        _ => Vec::new(),
    }
}

pub fn preview(
    request: &AgentOnboardingRequest,
    inspector: &dyn ProviderOnboarding,
) -> Result<AgentOnboardingPreview> {
    request.validate()?;
    let provider = AgentProviderConfiguration {
        backend: request.backend.clone(),
        profile_path: request.profile_path.clone(),
        model: request.model.clone(),
        reasoning_effort: request.reasoning_effort,
        config_metadata: None,
    };
    let inspection = inspector
        .inspect(
            &provider,
            request.execution_mode,
            &request.declared_capabilities,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
    if !inspection.authenticated {
        bail!("provider authentication was not verified")
    }
    Ok(AgentOnboardingPreview {
        agent: request.definition(&inspection),
        provider_capabilities: inspection.capabilities,
        permissions: request.permissions.clone(),
        authentication: AgentAuthentication {
            verified: inspection.authenticated,
            method: inspection.authentication_method,
            detail: inspection.authentication_detail,
        },
    })
}

pub fn document_from_storage(db: &Database, id: &str) -> Result<AgentConfigurationDocument> {
    let agent = db
        .get_global_agent(id)?
        .with_context(|| format!("agent '{id}' is not registered"))?;
    let authorization = db.agent_authorization(id)?;
    let permissions = db.agent_permissions(id)?;
    Ok(AgentConfigurationDocument {
        configuration_version: registry::AGENT_CONFIGURATION_VERSION,
        agent,
        permissions,
        authentication: AgentAuthentication {
            verified: authorization.as_ref().is_some_and(|v| v.authenticated),
            method: authorization
                .as_ref()
                .map_or_else(|| "unknown".into(), |v| v.authentication_method.clone()),
            detail: authorization.and_then(|v| v.authentication_detail),
        },
    })
}

pub fn validate_document(document: &AgentConfigurationDocument) -> Result<()> {
    if document.configuration_version != registry::AGENT_CONFIGURATION_VERSION {
        bail!(
            "unsupported agent configuration version {}",
            document.configuration_version
        )
    }
    if document.agent.model_version != registry::AGENT_MODEL_VERSION || !document.agent.is_global()
    {
        bail!("agent configuration has unsupported model version or ownership scope")
    }
    registry::validate_backend(document.agent.provider())?;
    if document.agent.roles.is_empty() {
        bail!("agent configuration must assign at least one Orc role")
    }
    if document.agent.execution_mode() == AgentExecutionMode::Automated
        && !matches!(
            document.agent.provider(),
            "codex" | "copilot" | "antigravity"
        )
    {
        bail!("automated agent configuration uses a backend that requires manual mode")
    }
    if (document.agent.execution.provider.model.is_some()
        || document.agent.execution.provider.reasoning_effort.is_some())
        && (document.agent.provider() != "codex"
            || document.agent.execution_mode() != AgentExecutionMode::Automated)
    {
        bail!("only automated Codex agents support model and reasoning-effort configuration")
    }
    Agent::from_definition(&document.agent.to_definition())?;
    if !document.authentication.verified {
        bail!("agent configuration has not verified provider authentication")
    }
    Ok(())
}

pub fn persist_preview(db: &Database, preview: &AgentOnboardingPreview) -> Result<Agent> {
    let agent = Agent::from_definition(&preview.agent)?;
    db.upsert_global_agent_configuration(
        &agent,
        &preview.permissions,
        &AgentAuthorization {
            authenticated: preview.authentication.verified,
            authentication_method: preview.authentication.method.clone(),
            authentication_detail: preview.authentication.detail.clone(),
        },
    )?;
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeCommandRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        output: ProviderCommandOutput,
    }

    impl ProviderCommandRunner for FakeCommandRunner {
        fn run(
            &self,
            executable: &str,
            args: &[&str],
            _cwd: &Path,
            _environment: Option<(&str, &Path)>,
        ) -> Result<ProviderCommandOutput, String> {
            self.calls.lock().unwrap().push((
                executable.into(),
                args.iter().map(|arg| (*arg).into()).collect(),
            ));
            Ok(ProviderCommandOutput {
                success: self.output.success,
                stdout: self.output.stdout.clone(),
                stderr: self.output.stderr.clone(),
            })
        }
    }

    #[test]
    fn automated_provider_authentication_uses_provider_login_checks() {
        for (backend, executable, args, method) in [
            ("copilot", "copilot", vec!["-p", "/user"], "copilot_user"),
            ("antigravity", "agy", vec!["models"], "antigravity_models"),
        ] {
            let (actual_executable, actual_args, actual_method) =
                authentication_check(backend).unwrap();
            assert_eq!(actual_executable, executable);
            assert_eq!(actual_args, args.as_slice());
            assert_eq!(actual_method, method);
        }
    }

    #[test]
    fn failed_provider_authentication_check_cannot_produce_authenticated_preview() {
        let runner = FakeCommandRunner {
            calls: Mutex::new(Vec::new()),
            output: ProviderCommandOutput {
                success: false,
                stdout: String::new(),
                stderr: "not logged in".into(),
            },
        };
        let onboarding = SystemProviderOnboarding::with_command_runner(".", Box::new(runner));
        let provider = AgentProviderConfiguration {
            backend: "copilot".into(),
            profile_path: None,
            model: None,
            reasoning_effort: None,
            config_metadata: None,
        };

        let result = onboarding.inspect(&provider, AgentExecutionMode::Automated, &[]);

        assert_eq!(result.unwrap_err(), "not logged in");
    }

    #[test]
    fn successful_provider_authentication_check_is_recorded_as_evidence() {
        let runner = FakeCommandRunner {
            calls: Mutex::new(Vec::new()),
            output: ProviderCommandOutput {
                success: true,
                stdout: "signed in as operator".into(),
                stderr: String::new(),
            },
        };
        let onboarding = SystemProviderOnboarding::with_command_runner(".", Box::new(runner));
        let provider = AgentProviderConfiguration {
            backend: "copilot".into(),
            profile_path: None,
            model: None,
            reasoning_effort: None,
            config_metadata: None,
        };

        let inspection = onboarding
            .inspect(&provider, AgentExecutionMode::Automated, &[])
            .unwrap();

        assert!(inspection.authenticated);
        assert_eq!(inspection.authentication_method, "copilot_user");
        assert_eq!(
            inspection.authentication_detail.as_deref(),
            Some("signed in as operator")
        );
    }
}
