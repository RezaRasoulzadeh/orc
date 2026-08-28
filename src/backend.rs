use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::lead::{LeadBackend, LeadBackendResponse, LeadContext};
use crate::registry::{
    Agent, AgentDefinition, AgentExecutionMode, AgentProviderConfiguration, QuotaLimits,
    ReasoningEffort,
};
use crate::storage::Database;
use crate::worker::{AntigravityWorker, CodexWorker, CopilotWorker, Worker};

/// Result of provider-owned onboarding inspection. The fields intentionally do
/// not include Orc roles or operator permissions.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderInspection {
    pub authenticated: bool,
    pub authentication_method: String,
    pub authentication_detail: Option<String>,
    pub capabilities: Vec<crate::registry::AgentCapability>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderExecutionOptions {
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub read_only: bool,
    pub executable: Option<PathBuf>,
}

impl ProviderExecutionOptions {
    pub fn from_agent(agent: &Agent) -> Self {
        Self {
            model: agent.execution.provider.model.clone(),
            reasoning_effort: agent.execution.provider.reasoning_effort,
            read_only: false,
            executable: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderQuotaSnapshot {
    pub remaining_percent: i64,
    pub reset_at: Option<i64>,
    pub limits: QuotaLimits,
}

#[derive(Clone, Debug)]
pub struct ProviderCommandOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait ProviderInspectionRunner: Send + Sync {
    fn run(
        &self,
        executable: &str,
        args: &[&str],
        cwd: &Path,
        environment: Option<(&str, &Path)>,
    ) -> Result<ProviderCommandOutput, String>;
}

/// Stable boundary between Orc's canonical agent contract and a provider.
///
/// An adapter translates an already validated Orc `Agent` into a worker. It
/// deliberately has no lifecycle methods: run status, reservations, task
/// transitions, and terminalization remain owned by Orc's application/storage
/// layers.
pub trait ProviderAdapter: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn supports_automated_execution(&self) -> bool {
        true
    }
    fn supports_execution_options(&self) -> bool {
        false
    }
    fn supports_quota(&self) -> bool {
        false
    }
    fn supports_lead(&self) -> bool {
        false
    }
    fn build_worker(&self, agent: &Agent) -> Result<Box<dyn Worker>, String> {
        self.build_worker_with_options(agent, &ProviderExecutionOptions::from_agent(agent))
    }
    fn build_worker_with_options(
        &self,
        agent: &Agent,
        options: &ProviderExecutionOptions,
    ) -> Result<Box<dyn Worker>, String>;
    fn build_lead(
        &self,
        agent: &AgentDefinition,
        repo_path: &Path,
        options: ProviderExecutionOptions,
    ) -> Result<Box<dyn LeadBackend>, String>;
    fn inspect(
        &self,
        provider: &AgentProviderConfiguration,
        mode: AgentExecutionMode,
        declared_capabilities: &[String],
        cwd: &Path,
        runner: &dyn ProviderInspectionRunner,
    ) -> Result<ProviderInspection, String>;
    fn check_health(
        &self,
        agent: &AgentDefinition,
        cwd: &Path,
        runner: &dyn HealthCommandRunner,
    ) -> Result<(), String>;
    fn sync_quota(
        &self,
        db: &Database,
        agent: &AgentDefinition,
    ) -> Result<ProviderQuotaSnapshot, String>;
}

pub struct CodexProviderAdapter;

impl ProviderAdapter for CodexProviderAdapter {
    fn provider_id(&self) -> &'static str {
        "codex"
    }

    fn supports_execution_options(&self) -> bool {
        true
    }

    fn supports_quota(&self) -> bool {
        true
    }

    fn supports_lead(&self) -> bool {
        true
    }

    fn build_worker(&self, agent: &Agent) -> Result<Box<dyn Worker>, String> {
        self.build_worker_with_options(agent, &ProviderExecutionOptions::from_agent(agent))
    }
    fn build_worker_with_options(
        &self,
        agent: &Agent,
        options: &ProviderExecutionOptions,
    ) -> Result<Box<dyn Worker>, String> {
        let profile_path = agent
            .execution
            .provider
            .profile_path
            .as_deref()
            .ok_or_else(|| {
                format!(
                    "Codex agent '{}' requires a configured profile path; run `orc agent profile {} <path>`",
                    agent.id, agent.id
                )
            })?;
        let worker = if options.read_only {
            CodexWorker::with_read_only_execution(
                PathBuf::from(profile_path),
                options.model.clone(),
                options.reasoning_effort,
            )
        } else {
            CodexWorker::with_execution(
                PathBuf::from(profile_path),
                options.model.clone(),
                options.reasoning_effort,
            )
        };
        let worker = match options.executable.clone() {
            Some(executable) => worker.with_executable(executable),
            None => worker,
        };
        Ok(Box::new(worker))
    }

    fn build_lead(
        &self,
        agent: &AgentDefinition,
        repo_path: &Path,
        options: ProviderExecutionOptions,
    ) -> Result<Box<dyn LeadBackend>, String> {
        Ok(Box::new(CodexLeadBackend::from_agent(
            agent,
            repo_path,
            options.model,
            options.reasoning_effort,
        )?))
    }

    fn inspect(
        &self,
        provider: &AgentProviderConfiguration,
        mode: AgentExecutionMode,
        declared_capabilities: &[String],
        cwd: &Path,
        runner: &dyn ProviderInspectionRunner,
    ) -> Result<ProviderInspection, String> {
        inspect_codex(provider, mode, declared_capabilities, cwd, runner)
    }

    fn check_health(
        &self,
        agent: &AgentDefinition,
        cwd: &Path,
        runner: &dyn HealthCommandRunner,
    ) -> Result<(), String> {
        check_codex_health(agent, cwd, runner)
    }

    fn sync_quota(
        &self,
        db: &Database,
        agent: &AgentDefinition,
    ) -> Result<ProviderQuotaSnapshot, String> {
        let snapshot = crate::codex_app_server::sync_agent(
            db,
            agent,
            &crate::codex_app_server::CodexAppServer,
        )?;
        Ok(ProviderQuotaSnapshot {
            remaining_percent: snapshot.remaining_percent,
            reset_at: snapshot.reset_at,
            limits: snapshot.limits,
        })
    }
}

pub struct CopilotProviderAdapter;

impl ProviderAdapter for CopilotProviderAdapter {
    fn provider_id(&self) -> &'static str {
        "copilot"
    }

    fn supports_execution_options(&self) -> bool {
        true
    }

    fn build_worker(&self, agent: &Agent) -> Result<Box<dyn Worker>, String> {
        self.build_worker_with_options(agent, &ProviderExecutionOptions::from_agent(agent))
    }
    fn build_worker_with_options(
        &self,
        agent: &Agent,
        options: &ProviderExecutionOptions,
    ) -> Result<Box<dyn Worker>, String> {
        let worker = CopilotWorker::with_execution(
            agent
                .execution
                .provider
                .profile_path
                .as_deref()
                .map(PathBuf::from),
            options.model.clone(),
            options.reasoning_effort,
        );
        let worker = match options.executable.clone() {
            Some(executable) => worker.with_executable(executable),
            None => worker,
        };
        Ok(Box::new(worker))
    }

    fn build_lead(
        &self,
        _agent: &AgentDefinition,
        _repo_path: &Path,
        _options: ProviderExecutionOptions,
    ) -> Result<Box<dyn LeadBackend>, String> {
        Err("provider does not support a read-only Lead boundary".into())
    }

    fn inspect(
        &self,
        provider: &AgentProviderConfiguration,
        mode: AgentExecutionMode,
        declared_capabilities: &[String],
        cwd: &Path,
        runner: &dyn ProviderInspectionRunner,
    ) -> Result<ProviderInspection, String> {
        inspect_command_provider(
            provider,
            mode,
            declared_capabilities,
            cwd,
            runner,
            ProviderAuthenticationCheck {
                executable: "copilot",
                args: &["-p", "/user"],
                authentication_method: "copilot_user",
            },
        )
    }

    fn check_health(
        &self,
        agent: &AgentDefinition,
        cwd: &Path,
        runner: &dyn HealthCommandRunner,
    ) -> Result<(), String> {
        check_command_health(agent, cwd, runner, "copilot", &["--version"], None)
    }

    fn sync_quota(
        &self,
        _db: &Database,
        agent: &AgentDefinition,
    ) -> Result<ProviderQuotaSnapshot, String> {
        Err(format!(
            "provider '{}' does not expose quota synchronization",
            agent.backend
        ))
    }
}

pub struct AntigravityProviderAdapter;

impl ProviderAdapter for AntigravityProviderAdapter {
    fn provider_id(&self) -> &'static str {
        "antigravity"
    }

    fn build_worker(&self, agent: &Agent) -> Result<Box<dyn Worker>, String> {
        self.build_worker_with_options(agent, &ProviderExecutionOptions::from_agent(agent))
    }
    fn build_worker_with_options(
        &self,
        _agent: &Agent,
        _options: &ProviderExecutionOptions,
    ) -> Result<Box<dyn Worker>, String> {
        Ok(Box::new(AntigravityWorker))
    }

    fn build_lead(
        &self,
        _agent: &AgentDefinition,
        _repo_path: &Path,
        _options: ProviderExecutionOptions,
    ) -> Result<Box<dyn LeadBackend>, String> {
        Err("provider does not support a read-only Lead boundary".into())
    }

    fn inspect(
        &self,
        provider: &AgentProviderConfiguration,
        mode: AgentExecutionMode,
        declared_capabilities: &[String],
        cwd: &Path,
        runner: &dyn ProviderInspectionRunner,
    ) -> Result<ProviderInspection, String> {
        inspect_command_provider(
            provider,
            mode,
            declared_capabilities,
            cwd,
            runner,
            ProviderAuthenticationCheck {
                executable: "agy",
                args: &["models"],
                authentication_method: "antigravity_models",
            },
        )
    }

    fn check_health(
        &self,
        agent: &AgentDefinition,
        cwd: &Path,
        runner: &dyn HealthCommandRunner,
    ) -> Result<(), String> {
        check_command_health(agent, cwd, runner, "agy", &["--version"], None)
    }

    fn sync_quota(
        &self,
        _db: &Database,
        agent: &AgentDefinition,
    ) -> Result<ProviderQuotaSnapshot, String> {
        Err(format!(
            "provider '{}' does not expose quota synchronization",
            agent.backend
        ))
    }
}

pub fn provider_adapter(provider: &str) -> Option<Box<dyn ProviderAdapter>> {
    match provider {
        "codex" => Some(Box::new(CodexProviderAdapter)),
        "copilot" => Some(Box::new(CopilotProviderAdapter)),
        "antigravity" => Some(Box::new(AntigravityProviderAdapter)),
        _ => None,
    }
}

pub fn provider_supports_automated_execution(provider: &str) -> bool {
    provider_adapter(provider).is_some_and(|adapter| adapter.supports_automated_execution())
}

pub fn provider_supports_execution_options(provider: &str) -> bool {
    provider_adapter(provider).is_some_and(|adapter| adapter.supports_execution_options())
}

pub fn provider_supports_quota(provider: &str) -> bool {
    provider_adapter(provider).is_some_and(|adapter| adapter.supports_quota())
}

pub fn sync_agent_quota(
    db: &Database,
    agent: &AgentDefinition,
) -> Result<ProviderQuotaSnapshot, String> {
    provider_adapter(&agent.backend)
        .ok_or_else(|| format!("unsupported backend '{}'", agent.backend))?
        .sync_quota(db, agent)
}

pub fn provider_capabilities(backend: &str) -> Vec<crate::registry::AgentCapability> {
    match backend {
        "codex" | "antigravity" => vec![
            crate::registry::AgentCapability::Code,
            crate::registry::AgentCapability::RepositoryRead,
            crate::registry::AgentCapability::RepositoryWrite,
            crate::registry::AgentCapability::CommandExecution,
            crate::registry::AgentCapability::StructuredOutput,
        ],
        // Copilot CLI's documented `-s` mode returns plain text. It does not
        // expose Orc's structured provider protocol, so that capability is
        // intentionally absent even though the generic worker protocol can
        // still ask it to follow a text prompt.
        "copilot" => vec![
            crate::registry::AgentCapability::Code,
            crate::registry::AgentCapability::RepositoryRead,
            crate::registry::AgentCapability::RepositoryWrite,
            crate::registry::AgentCapability::CommandExecution,
            crate::registry::AgentCapability::Streaming,
            crate::registry::AgentCapability::Cancellation,
        ],
        _ => Vec::new(),
    }
}

fn inspect_command_provider(
    provider: &AgentProviderConfiguration,
    mode: AgentExecutionMode,
    declared_capabilities: &[String],
    cwd: &Path,
    runner: &dyn ProviderInspectionRunner,
    check: ProviderAuthenticationCheck<'_>,
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
                .map(|value| crate::registry::AgentCapability::parse(value))
                .collect(),
        });
    }
    let output = runner.run(check.executable, check.args, cwd, None)?;
    if !output.success {
        let detail = output.stderr.trim().to_owned();
        return Err(if detail.is_empty() {
            format!("{} authentication check failed", check.executable)
        } else {
            detail
        });
    }
    Ok(ProviderInspection {
        authenticated: true,
        authentication_method: check.authentication_method.into(),
        authentication_detail: Some(output.stdout.trim().to_owned()),
        capabilities: provider_capabilities(&provider.backend),
    })
}

struct ProviderAuthenticationCheck<'a> {
    executable: &'a str,
    args: &'a [&'a str],
    authentication_method: &'a str,
}

fn inspect_codex(
    provider: &AgentProviderConfiguration,
    mode: AgentExecutionMode,
    declared_capabilities: &[String],
    cwd: &Path,
    runner: &dyn ProviderInspectionRunner,
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
                .map(|value| crate::registry::AgentCapability::parse(value))
                .collect(),
        });
    }
    let profile = provider
        .profile_path
        .as_deref()
        .ok_or_else(|| "Codex onboarding requires a configured profile path".to_owned())?;
    if !Path::new(profile).is_dir() {
        return Err(format!("profile path does not exist: {profile}"));
    }
    let output = runner.run(
        "codex",
        &["login", "status"],
        cwd,
        Some(("CODEX_HOME", Path::new(profile))),
    )?;
    if !output.success {
        let detail = output.stderr.trim().to_owned();
        return Err(if detail.is_empty() {
            "codex authentication check failed".into()
        } else {
            detail
        });
    }
    Ok(ProviderInspection {
        authenticated: true,
        authentication_method: "codex_login_status".into(),
        authentication_detail: Some(output.stdout.trim().to_owned()),
        capabilities: provider_capabilities("codex"),
    })
}

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
    provider_adapter(&agent.backend)
        .ok_or_else(|| format!("unsupported backend '{}'", agent.backend))?
        .check_health(agent, cwd, runner)
}

fn check_command_health(
    agent: &AgentDefinition,
    cwd: &Path,
    runner: &dyn HealthCommandRunner,
    executable: &str,
    args: &[&str],
    environment: Option<(&str, &Path)>,
) -> Result<(), String> {
    if !runner.executable_exists(executable) {
        return Err(format!("provider CLI '{executable}' not found"));
    }
    runner
        .run(executable, args, cwd, environment)
        .map_err(|error| format!("{}: {error}", agent.id))
}

fn check_codex_health(
    agent: &AgentDefinition,
    cwd: &Path,
    runner: &dyn HealthCommandRunner,
) -> Result<(), String> {
    let profile = agent.profile_path.as_deref().map(Path::new).ok_or_else(|| {
        format!(
            "Codex agent '{}' requires a configured profile path; run `orc agent profile {} <path>`",
            agent.id, agent.id
        )
    })?;
    if !profile.is_dir() {
        return Err(format!(
            "profile path does not exist: {}",
            profile.display()
        ));
    }
    check_command_health(
        agent,
        cwd,
        runner,
        "codex",
        &["login", "status"],
        Some(("CODEX_HOME", profile)),
    )
}

/// Codex's Lead transport and response decoding live with the Codex adapter.
/// Orc's Lead service only sees the provider-neutral `LeadBackend` contract.
pub struct CodexLeadBackend {
    profile_path: Option<PathBuf>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    repo_path: PathBuf,
}

impl CodexLeadBackend {
    pub fn from_agent(
        agent: &AgentDefinition,
        repo_path: &Path,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Self, String> {
        if agent.backend != "codex" {
            return Err(format!(
                "Lead provider agent '{}' has an incompatible provider",
                agent.id
            ));
        }
        Ok(Self {
            profile_path: agent.profile_path.as_deref().map(PathBuf::from),
            model: model.or_else(|| agent.model.clone()),
            reasoning_effort: reasoning_effort.or(agent.reasoning_effort),
            repo_path: repo_path.to_path_buf(),
        })
    }

    pub fn command_args(&self, prompt: &str) -> Vec<String> {
        let mut args = CodexWorker::command_args_with_execution(
            prompt,
            self.model.as_deref(),
            self.reasoning_effort,
        );
        if let Some(sandbox) = args
            .iter_mut()
            .find(|arg| arg.as_str() == "workspace-write")
        {
            *sandbox = "read-only".into();
        }
        args
    }

    fn prompt(context: &LeadContext, message: &str) -> Result<String, String> {
        let context = serde_json::to_string(context)
            .map_err(|error| format!("failed to serialize Lead context: {error}"))?;
        Ok(format!(
            "You are Orc's project Lead. You are strictly read-only: inspect the supplied persisted project and repository state only. You must not edit files, create commits, create or apply tasks, invoke Planner, dispatch, review, revise, or accept work. Return exactly one decision with kind DIRECT_TASKS, PLAN_REQUIRED, or USER_DECISION_REQUIRED, plus a message. Proposals are optional human-gated suggestions and are never applied by Lead. Respond with only structured JSON.\nProject context:\n{context}\nUser message:\n{message}"
        ))
    }

    pub fn parse_response(output: &str) -> Result<LeadBackendResponse, String> {
        serde_json::from_str(output.trim())
            .map_err(|error| format!("Lead provider returned malformed structured output: {error}"))
    }
}

impl LeadBackend for CodexLeadBackend {
    fn invoke(&self, context: &LeadContext, message: &str) -> Result<LeadBackendResponse, String> {
        let prompt = Self::prompt(context, message)?;
        let mut command = Command::new("codex");
        command.args(self.command_args(&prompt));
        if let Some(profile_path) = &self.profile_path {
            apply_profile_environment(&mut command, profile_path);
        }
        configure_noninteractive(&mut command, &self.repo_path);
        let output = crate::worker::run_command_with_timeout(
            command,
            crate::worker::configured_timeout(
                "ORC_LEAD_TIMEOUT_SECS",
                crate::worker::DEFAULT_WORKER_TIMEOUT,
            ),
        )?;
        if !output.status.success() {
            return Err(format!(
                "Codex Lead exited with non-zero status: {}",
                output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        let stdout = String::from_utf8(output.stdout)
            .map_err(|error| format!("Codex Lead returned non-UTF-8 output: {error}"))?;
        Self::parse_response(&stdout)
    }
}

pub struct WorkerFactory;

impl WorkerFactory {
    pub fn build_global(agent: &Agent) -> Result<Box<dyn Worker>, String> {
        if agent.model_version != crate::registry::AGENT_MODEL_VERSION {
            return Err(format!(
                "unsupported agent model version {}",
                agent.model_version
            ));
        }
        if !agent.is_global() {
            return Err(format!("agent '{}' is not globally owned", agent.id));
        }
        if !agent.is_available() {
            return Err(format!("agent '{}' is not available", agent.id));
        }
        if agent.execution_mode() != AgentExecutionMode::Automated {
            return Err(format!(
                "agent '{}' is not configured for automated execution",
                agent.id
            ));
        }
        let adapter = provider_adapter(agent.provider())
            .ok_or_else(|| format!("unsupported agent backend '{}'", agent.provider()))?;
        adapter.build_worker_with_options(agent, &ProviderExecutionOptions::from_agent(agent))
    }

    pub fn build_lead(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String> {
        Self::build_read_only(agent, model, reasoning_effort, None)
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
        Self::build_read_only(agent, model, reasoning_effort, executable)
    }

    pub fn build_read_only(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
        executable: Option<PathBuf>,
    ) -> Result<Box<dyn Worker>, String> {
        let canonical = Agent::from_definition(agent).map_err(|error| error.to_string())?;
        let adapter = provider_adapter(canonical.provider())
            .ok_or_else(|| format!("unsupported agent backend '{}'", canonical.provider()))?;
        if !adapter.supports_lead() {
            return Err(format!(
                "provider '{}' has no read-only execution boundary",
                canonical.provider()
            ));
        }
        adapter.build_worker_with_options(
            &canonical,
            &ProviderExecutionOptions {
                model: model.or(canonical.execution.provider.model.clone()),
                reasoning_effort: reasoning_effort
                    .or(canonical.execution.provider.reasoning_effort),
                read_only: true,
                executable,
            },
        )
    }

    pub fn build(agent: &AgentDefinition) -> Result<Box<dyn Worker>, String> {
        let canonical = Agent::from_definition(agent).map_err(|error| error.to_string())?;
        Self::build_global(&canonical)
    }

    pub fn build_with_overrides(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String> {
        let canonical = Agent::from_definition(agent).map_err(|error| error.to_string())?;
        let adapter = provider_adapter(canonical.provider())
            .ok_or_else(|| format!("unsupported agent backend '{}'", canonical.provider()))?;
        if (model.is_some() || reasoning_effort.is_some()) && !adapter.supports_execution_options()
        {
            return Err(format!(
                "backend '{}' does not support execution overrides",
                agent.backend
            ));
        }
        adapter.build_worker_with_options(
            &canonical,
            &ProviderExecutionOptions {
                model: model.or(canonical.execution.provider.model.clone()),
                reasoning_effort: reasoning_effort
                    .or(canonical.execution.provider.reasoning_effort),
                read_only: false,
                executable: None,
            },
        )
    }

    pub fn build_with_codex_overrides(
        agent: &AgentDefinition,
        model: Option<String>,
        reasoning_effort: Option<ReasoningEffort>,
    ) -> Result<Box<dyn Worker>, String> {
        Self::build_with_overrides(agent, model, reasoning_effort)
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
