use serde::{Deserialize, Serialize};

use crate::{state::OrcState, task::TaskPriority};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct EngineeringLeadRequest {
    pub protocol_version: u32,
    pub project: String,
    pub cto_request: String,
    pub active_tasks: Vec<TaskSummary>,
}

impl EngineeringLeadRequest {
    pub fn from_state(cto_request: String, state: &OrcState) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            project: state.project.clone(),
            cto_request,
            active_tasks: state
                .tasks
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LeadAction {
    CreateTask {
        title: String,
        objective: String,
        role: String,
        priority: TaskPriority,
    },
    RequireCtoApproval {
        reason: String,
    },
}
