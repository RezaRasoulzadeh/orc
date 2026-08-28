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
                ..
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
                    cancellation_reason: None,
                    required_capabilities: Vec::new(),
                    scope_mode: None,
                    context_files: Vec::new(),
                    expected_changes: Vec::new(),
                    reasoning_effort: None,
                    effort_reason: None,
                    risk_factors: Vec::new(),
                });
            }
            LeadAction::RequireCtoApproval { reason } => {
                self.pending_cto_approvals.push(reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::LeadAction;

    #[test]
    fn create_task_increments_and_adds() {
        let mut s = OrcState::new("proj");
        assert_eq!(s.next_task_id, 1);

        s.apply_action(LeadAction::CreateTask {
            title: "Implement widget".into(),
            objective: "Add widget to UI".into(),
            role: "developer".into(),
            priority: crate::task::TaskPriority::Normal,
            scope_mode: None,
            context_files: Vec::new(),
            expected_changes: Vec::new(),
        });

        assert_eq!(s.tasks.len(), 1);
        assert_eq!(s.next_task_id, 2);
        let t = &s.tasks[0];
        assert_eq!(t.id, "T-0001");
        assert_eq!(t.title, "Implement widget");
        assert_eq!(t.status, TaskStatus::Backlog);
    }

    #[test]
    fn require_cto_approval_appends_reason() {
        let mut s = OrcState::new("proj");
        s.apply_action(LeadAction::RequireCtoApproval {
            reason: "security review".into(),
        });
        assert_eq!(s.pending_cto_approvals.len(), 1);
        assert_eq!(s.pending_cto_approvals[0], "security review");
    }
}
