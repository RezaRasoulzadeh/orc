use anyhow::{Result, bail};

use crate::storage::Database;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotaLimit {
    pub remaining_percent: i64,
    pub reset_at: Option<i64>,
    pub window_duration_mins: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuotaLimits {
    pub primary: Option<QuotaLimit>,
    pub secondary: Option<QuotaLimit>,
    #[serde(default)]
    pub monthly: Option<QuotaLimit>,
    pub effective: String,
}

pub const AVAILABLE: &str = "available";
pub const UNAVAILABLE: &str = "unavailable";
pub const AUTOMATED: &str = "automated";
pub const MANUAL: &str = "manual";

#[derive(Clone, Debug, PartialEq, Eq)]
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
