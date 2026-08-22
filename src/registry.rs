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
}

impl AgentDefinition {
    pub fn supports(&self, required: &[String]) -> bool {
        required
            .iter()
            .all(|capability| self.capabilities.iter().any(|item| item == capability))
    }

    pub fn is_selectable(&self, required: &[String]) -> bool {
        self.enabled && self.status == AVAILABLE && self.supports(required)
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
