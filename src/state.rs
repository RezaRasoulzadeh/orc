use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    protocol::LeadAction,
    task::{Task, TaskStatus},
};

const STATE_PATH: &str = ".orc/state.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct OrcState {
    pub project: String,
    pub next_task_id: u64,
    pub tasks: Vec<Task>,
    pub pending_cto_approvals: Vec<String>,
}

impl OrcState {
    pub fn new(project: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            next_task_id: 1,
            tasks: Vec::new(),
            pending_cto_approvals: Vec::new(),
        }
    }

    pub fn load() -> Result<Self> {
        let data = fs::read_to_string(STATE_PATH)
            .with_context(|| format!("failed to read {STATE_PATH}; run `orc init` first"))?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = Path::new(STATE_PATH).parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(STATE_PATH, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub fn apply_action(&mut self, action: LeadAction) {
        match action {
            LeadAction::CreateTask {
                title,
                objective,
                role,
                priority,
            } => {
                let id = format!("T-{:04}", self.next_task_id);
                self.next_task_id += 1;
                self.tasks.push(Task {
                    id,
                    title,
                    objective,
                    role,
                    priority,
                    status: TaskStatus::Backlog,
                });
            }
            LeadAction::RequireCtoApproval { reason } => {
                self.pending_cto_approvals.push(reason);
            }
        }
    }
}
