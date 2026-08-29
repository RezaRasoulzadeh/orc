use anyhow::{Result, bail};
use std::collections::BTreeMap;

use crate::storage::Database;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotaLimit {
    pub remaining_percent: i64,
    pub reset_at: Option<i64>,
    pub window_duration_mins: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IndividualQuotaLimit {
    pub limit: String,
    pub used: String,
    pub remaining_percent: i64,
    pub reset_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotaLimitBucket {
    pub primary: Option<QuotaLimit>,
    pub secondary: Option<QuotaLimit>,
    pub individual_limit: Option<IndividualQuotaLimit>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotaLimits {
    pub primary: Option<QuotaLimit>,
    pub secondary: Option<QuotaLimit>,
    #[serde(default)]
    pub individual_limit: Option<IndividualQuotaLimit>,
    #[serde(default)]
    pub by_limit_id: BTreeMap<String, QuotaLimitBucket>,
    pub effective: String,
}

pub const AVAILABLE: &str = "available";
pub const UNAVAILABLE: &str = "unavailable";
pub const AUTOMATED: &str = "automated";
pub const MANUAL: &str = "manual";
pub const AGENT_MODEL_VERSION: u16 = 1;
pub const GLOBAL_AGENT_SCOPE: &str = "global";
pub const AGENT_CONFIGURATION_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AgentAction {
    Code,
    Review,
    Plan,
    Lead,
}

/// Stable Orc roles. `AgentAction` is retained as the storage and CLI name;
/// this alias makes the role contract explicit without duplicating it.
pub type AgentRole = AgentAction;

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    Code,
    Review,
    Plan,
    Lead,
    RepositoryRead,
    RepositoryWrite,
    CommandExecution,
    StructuredOutput,
    Streaming,
    Cancellation,
    Custom(String),
}

/// Permissions granted by the operator to an onboarded provider. These are
/// deliberately separate from provider capabilities and Orc roles: a
/// provider may support an operation without Orc granting it, and a role is
/// an Orc scheduling authority rather than a provider feature.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorPermission {
    RepositoryRead,
    RepositoryWrite,
    CommandExecution,
    NetworkAccess,
    Custom(String),
}

impl OperatorPermission {
    pub fn parse(value: &str) -> Self {
        let normalized = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        match normalized.as_str() {
            "read" | "repository_read" | "repo_read" => Self::RepositoryRead,
            "write" | "repository_write" | "repo_write" => Self::RepositoryWrite,
            "command" | "terminal" | "command_execution" => Self::CommandExecution,
            "network" | "network_access" => Self::NetworkAccess,
            _ => Self::Custom(normalized),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::RepositoryRead => "repository_read",
            Self::RepositoryWrite => "repository_write",
            Self::CommandExecution => "command_execution",
            Self::NetworkAccess => "network_access",
            Self::Custom(value) => value,
        }
    }
}

impl AgentCapability {
    pub fn parse(value: &str) -> Self {
        let normalized = value.trim().to_ascii_lowercase().replace([' ', '-'], "_");
        match normalized.as_str() {
            "code" | "coding" => Self::Code,
            "review" | "reviewing" => Self::Review,
            "plan" | "planning" => Self::Plan,
            "lead" => Self::Lead,
            "read" | "repository_read" | "repo_read" => Self::RepositoryRead,
            "write" | "repository_write" | "repo_write" => Self::RepositoryWrite,
            "terminal" | "command" | "commands" | "command_execution" => Self::CommandExecution,
            "structured" | "structured_output" => Self::StructuredOutput,
            "stream" | "streaming" => Self::Streaming,
            "cancel" | "cancellation" => Self::Cancellation,
            _ => Self::Custom(normalized),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Code => "code",
            Self::Review => "review",
            Self::Plan => "plan",
            Self::Lead => "lead",
            Self::RepositoryRead => "repository_read",
            Self::RepositoryWrite => "repository_write",
            Self::CommandExecution => "command_execution",
            Self::StructuredOutput => "structured_output",
            Self::Streaming => "streaming",
            Self::Cancellation => "cancellation",
            Self::Custom(value) => value,
        }
    }
}

pub fn normalize_capability_names(values: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    values
        .iter()
        .map(|value| AgentCapability::parse(value))
        .filter(|capability| seen.insert(capability.clone()))
        .map(|capability| capability.as_str().to_owned())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionMode {
    Automated,
    Manual,
}

impl AgentExecutionMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Automated => AUTOMATED,
            Self::Manual => MANUAL,
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            AUTOMATED => Ok(Self::Automated),
            MANUAL => Ok(Self::Manual),
            _ => bail!("invalid agent execution mode '{value}'"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Available,
    Unavailable,
    Archived,
}

impl AgentLifecycleState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => AVAILABLE,
            Self::Unavailable => UNAVAILABLE,
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            AVAILABLE => Ok(Self::Available),
            UNAVAILABLE => Ok(Self::Unavailable),
            "archived" => Ok(Self::Archived),
            _ => bail!("invalid agent lifecycle state '{value}'"),
        }
    }

    /// Lifecycle transitions are owned by Orc. Provider adapters never receive
    /// this state and therefore cannot complete, fail, or archive an agent.
    pub fn can_transition_to(self, target: Self) -> bool {
        match (self, target) {
            (Self::Archived, _) => false,
            (Self::Available, Self::Available)
            | (Self::Available, Self::Unavailable)
            | (Self::Available, Self::Archived)
            | (Self::Unavailable, Self::Available)
            | (Self::Unavailable, Self::Unavailable)
            | (Self::Unavailable, Self::Archived) => true,
        }
    }

    pub fn transition(self, target: Self) -> Result<Self> {
        if self.can_transition_to(target) {
            Ok(target)
        } else {
            bail!("invalid agent lifecycle transition: {self:?} -> {target:?}")
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentProviderConfiguration {
    pub backend: String,
    pub profile_path: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub config_metadata: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentExecution {
    pub mode: AgentExecutionMode,
    pub provider: AgentProviderConfiguration,
}

/// Canonical, globally owned Orc agent contract.
///
/// `AgentDefinition` remains the database/CLI compatibility representation.
/// New orchestration and provider code should use this model so provider
/// configuration cannot become an implicit owner of Orc lifecycle state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Agent {
    pub model_version: u16,
    pub scope: String,
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub priority: i64,
    pub roles: Vec<AgentRole>,
    pub capabilities: Vec<AgentCapability>,
    pub execution: AgentExecution,
    pub lifecycle: AgentLifecycleState,
    pub unavailable_reason: Option<String>,
    #[serde(default)]
    pub quota_remaining_percent: Option<i64>,
    #[serde(default)]
    pub quota_reset_at: Option<String>,
    #[serde(default)]
    pub quota_checked_at: Option<String>,
    #[serde(default)]
    pub quota_source: Option<String>,
    #[serde(default)]
    pub quota_limits: Option<QuotaLimits>,
}

impl Agent {
    pub fn from_definition(definition: &AgentDefinition) -> Result<Self> {
        Ok(Self {
            model_version: AGENT_MODEL_VERSION,
            scope: GLOBAL_AGENT_SCOPE.to_owned(),
            id: definition.id.clone(),
            display_name: definition.display_name.clone(),
            enabled: definition.enabled,
            priority: definition.priority,
            roles: definition.actions.clone(),
            capabilities: definition
                .capabilities
                .iter()
                .map(|value| AgentCapability::parse(value))
                .collect(),
            execution: AgentExecution {
                mode: AgentExecutionMode::parse(&definition.execution_mode)?,
                provider: AgentProviderConfiguration {
                    backend: definition.backend.clone(),
                    profile_path: definition.profile_path.clone(),
                    model: definition.model.clone(),
                    reasoning_effort: definition.reasoning_effort,
                    config_metadata: definition.config_metadata.clone(),
                },
            },
            lifecycle: AgentLifecycleState::parse(&definition.status)?,
            unavailable_reason: definition.unavailable_reason.clone(),
            quota_remaining_percent: definition.quota_remaining_percent,
            quota_reset_at: definition.quota_reset_at.clone(),
            quota_checked_at: definition.quota_checked_at.clone(),
            quota_source: definition.quota_source.clone(),
            quota_limits: definition.quota_limits.clone(),
        })
    }

    pub fn is_global(&self) -> bool {
        self.scope == GLOBAL_AGENT_SCOPE
    }

    pub fn supports(&self, required: &[AgentCapability]) -> bool {
        required
            .iter()
            .all(|capability| self.capabilities.contains(capability))
    }

    pub fn supports_named_capabilities(&self, required: &[String]) -> bool {
        let normalized = required
            .iter()
            .map(|value| AgentCapability::parse(value))
            .collect::<Vec<_>>();
        self.supports(&normalized)
    }

    pub fn normalized_capabilities(&self) -> Vec<String> {
        self.capabilities
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect()
    }

    pub fn provider(&self) -> &str {
        &self.execution.provider.backend
    }

    pub fn execution_mode(&self) -> AgentExecutionMode {
        self.execution.mode
    }

    pub fn is_available(&self) -> bool {
        self.enabled && self.lifecycle == AgentLifecycleState::Available
    }

    pub fn to_definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: self.id.clone(),
            backend: self.execution.provider.backend.clone(),
            execution_mode: self.execution.mode.as_str().to_owned(),
            display_name: self.display_name.clone(),
            enabled: self.enabled,
            priority: self.priority,
            capabilities: self
                .capabilities
                .iter()
                .map(|capability| capability.as_str().to_owned())
                .collect(),
            status: self.lifecycle.as_str().to_owned(),
            unavailable_reason: self.unavailable_reason.clone(),
            profile_path: self.execution.provider.profile_path.clone(),
            model: self.execution.provider.model.clone(),
            reasoning_effort: self.execution.provider.reasoning_effort,
            config_metadata: self.execution.provider.config_metadata.clone(),
            quota_remaining_percent: self.quota_remaining_percent,
            quota_reset_at: self.quota_reset_at.clone(),
            quota_checked_at: self.quota_checked_at.clone(),
            quota_source: self.quota_source.clone(),
            quota_limits: self.quota_limits.clone(),
            actions: self.roles.clone(),
        }
    }
}

impl AgentAction {
    pub const fn all() -> [Self; 4] {
        [Self::Code, Self::Review, Self::Plan, Self::Lead]
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Review => "review",
            Self::Plan => "plan",
            Self::Lead => "lead",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "code" | "coder" => Ok(Self::Code),
            "review" | "reviewer" => Ok(Self::Review),
            "plan" | "planner" => Ok(Self::Plan),
            "lead" => Ok(Self::Lead),
            _ => bail!("invalid agent action '{value}'; expected code, review, plan, or lead"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentActionProfile {
    pub action: AgentAction,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Debug, serde::Deserialize)]
struct AgentConfigMetadata {
    manual_workspace_url: Option<String>,
}

pub fn manual_workspace_url(agent: &AgentDefinition) -> Result<Option<String>> {
    if agent.execution_mode != MANUAL {
        return Ok(None);
    }
    if let Some(metadata) = &agent.config_metadata {
        let metadata: AgentConfigMetadata = serde_json::from_str(metadata).map_err(|error| {
            anyhow::anyhow!("invalid config_metadata for agent '{}': {error}", agent.id)
        })?;
        if let Some(url) = metadata.manual_workspace_url {
            return Ok(Some(url));
        }
    }
    Ok(match agent.backend.as_str() {
        "chatgpt" => Some("https://chatgpt.com/".into()),
        "claude" => Some("https://claude.ai/".into()),
        _ => None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

impl ReasoningEffort {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => bail!(
                "invalid reasoning effort '{value}'; expected one of: none, low, medium, high"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub const fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::None | Self::Low => Self::Medium,
            Self::Medium | Self::High => Self::High,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentDefinition {
    pub id: String,
    pub backend: String,
    pub execution_mode: String,
    pub display_name: String,
    pub enabled: bool,
    pub priority: i64,
    pub capabilities: Vec<String>,
    pub status: String,
    pub unavailable_reason: Option<String>,
    pub profile_path: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub config_metadata: Option<String>,
    pub quota_remaining_percent: Option<i64>,
    pub quota_reset_at: Option<String>,
    pub quota_checked_at: Option<String>,
    pub quota_source: Option<String>,
    pub quota_limits: Option<QuotaLimits>,
    #[serde(default)]
    pub actions: Vec<AgentAction>,
}

impl AgentDefinition {
    pub fn supports(&self, required: &[String]) -> bool {
        required
            .iter()
            .map(|capability| AgentCapability::parse(capability))
            .all(|capability| {
                self.capabilities
                    .iter()
                    .map(|item| AgentCapability::parse(item))
                    .any(|item| item == capability)
            })
    }

    pub fn is_selectable(&self, required: &[String]) -> bool {
        self.enabled && self.status == AVAILABLE && self.supports(required)
    }

    pub fn supports_action(&self, action: AgentAction) -> bool {
        self.actions.contains(&action)
    }
}

pub fn select_agent<'a>(
    agents: &'a [AgentDefinition],
    required_capabilities: &[String],
) -> Result<&'a AgentDefinition> {
    agents
        .iter()
        .filter(|agent| agent.is_selectable(required_capabilities))
        .max_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| right.id.cmp(&left.id))
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no enabled, available agent satisfies required capabilities: {}",
                required_capabilities.join(", ")
            )
        })
}

pub fn select_agent_for_action<'a>(
    agents: &'a [AgentDefinition],
    action: AgentAction,
    required_capabilities: &[String],
) -> Result<&'a AgentDefinition> {
    agents
        .iter()
        .filter(|agent| {
            agent.execution_mode == AUTOMATED
                && agent.is_selectable(required_capabilities)
                && agent.supports_action(action)
        })
        .max_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| right.id.cmp(&left.id))
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no enabled, available agent supports action '{}'",
                action.as_str()
            )
        })
}

pub fn get_agent(db: &Database, id: &str) -> Result<AgentDefinition> {
    db.get_agent(id)?
        .ok_or_else(|| anyhow::anyhow!("agent '{}' is not registered", id))
}

pub fn validate_backend(backend: &str) -> Result<()> {
    match backend {
        "copilot" | "codex" | "antigravity" | "chatgpt" | "claude" | "generic_manual" => Ok(()),
        _ => bail!(
            "unsupported agent backend '{}'; supported backends: copilot, codex, antigravity",
            backend
        ),
    }
}
