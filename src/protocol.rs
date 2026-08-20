use serde::{Deserialize, Serialize};

use crate::task::{Task, TaskPriority, TaskScopeMode};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectDiscoveryRequest {
    pub protocol_version: u32,
    pub project_name: String,
    pub repository_path: String,
    pub engineering_contract: String,
    pub instructions: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectDiscoveryResponse {
    pub protocol_version: u32,
    pub project: DiscoveryProject,
    pub architecture: DiscoveryArchitecture,
    pub engineering: DiscoveryEngineering,
    pub state: DiscoveryState,
    pub unknowns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryProject {
    pub name: String,
    pub purpose: String,
    pub languages: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryArchitecture {
    pub entry_points: Vec<String>,
    pub modules: Vec<String>,
    pub boundaries: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryEngineering {
    pub build_commands: Vec<String>,
    pub test_commands: Vec<String>,
    pub observed_patterns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DiscoveryState {
    pub implemented: Vec<String>,
    pub in_progress: Vec<String>,
    pub risks: Vec<String>,
}

impl ProjectDiscoveryResponse {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            anyhow::bail!(
                "unsupported discovery protocol version {}; expected {}",
                self.protocol_version,
                PROTOCOL_VERSION
            );
        }
        if self.project.name.trim().is_empty() {
            anyhow::bail!("discovery response project name must not be empty");
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EngineeringLeadRequest {
    pub protocol_version: u32,
    pub project: String,
    pub cto_request: String,
    pub active_tasks: Vec<TaskSummary>,
}

impl EngineeringLeadRequest {
    pub fn from_tasks(cto_request: String, project: String, tasks: &[Task]) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            project,
            cto_request,
            active_tasks: tasks
                .iter()
                .filter(|task| !task.status.is_terminal())
                .map(|task| TaskSummary {
                    id: task.id.clone(),
                    title: task.title.clone(),
                    status: task.status.to_string(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EngineeringLeadResponse {
    pub protocol_version: u32,
    pub message_to_cto: Option<String>,
    pub actions: Vec<LeadAction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LeadAction {
    CreateTask {
        title: String,
        objective: String,
        role: String,
        priority: TaskPriority,
        #[serde(default)]
        scope_mode: Option<TaskScopeMode>,
        #[serde(default)]
        context_files: Vec<String>,
        #[serde(default)]
        expected_changes: Vec<String>,
    },
    RequireCtoApproval {
        reason: String,
    },
}
