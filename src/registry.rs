use anyhow::{Result, bail};

use crate::storage::Database;

pub const AVAILABLE: &str = "available";
pub const UNAVAILABLE: &str = "unavailable";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentDefinition {
    pub id: String,
    pub backend: String,
    pub display_name: String,
    pub enabled: bool,
    pub priority: i64,
    pub capabilities: Vec<String>,
    pub status: String,
    pub unavailable_reason: Option<String>,
    pub profile_path: Option<String>,
    pub config_metadata: Option<String>,
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
        "copilot" | "codex" => Ok(()),
        _ => bail!(
            "unsupported agent backend '{}'; supported backends: copilot, codex",
            backend
        ),
    }
}
