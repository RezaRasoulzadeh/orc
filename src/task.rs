use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct CreateTaskInput {
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    pub required_capabilities: Vec<String>,
    pub scope_mode: Option<TaskScopeMode>,
    pub context_files: Vec<String>,
    pub expected_changes: Vec<String>,
    pub dependencies: Vec<String>,
}

/// The contract fields persisted with a Task and consumed by Worker PREPARE.
/// Planner proposal metadata may populate these fields when a task is created,
/// but it is not consulted during execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskContract {
    pub unchanged: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    pub validation: Vec<String>,
}

impl TaskContract {
    pub fn defaults(objective: &str) -> Self {
        Self {
            unchanged: Vec::new(),
            acceptance_criteria: vec![objective.to_owned()],
            required_tests: vec!["configured validation pipeline".into()],
            validation: vec!["configured validation evidence".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    pub status: TaskStatus,
    #[serde(default)]
    pub cancellation_reason: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[serde(default)]
    pub scope_mode: Option<TaskScopeMode>,
    #[serde(default)]
    pub context_files: Vec<String>,
    #[serde(default)]
    pub expected_changes: Vec<String>,
    #[serde(default)]
    pub reasoning_effort: Option<crate::registry::ReasoningEffort>,
    #[serde(default)]
    pub effort_reason: Option<String>,
    #[serde(default)]
    pub risk_factors: Vec<crate::protocol::TaskRiskFactor>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskScopeMode {
    Focused,
    Module,
    Project,
}

impl TaskScopeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "focused" => Some(Self::Focused),
            "module" => Some(Self::Module),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}

impl fmt::Display for TaskScopeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Focused => "focused",
            Self::Module => "module",
            Self::Project => "project",
        })
    }
}

impl Task {
    pub const DEFAULT_REQUIRED_CAPABILITIES: [&'static str; 2] = ["code", "command_execution"];
    pub const DEFAULT_REASONING_EFFORT: crate::registry::ReasoningEffort =
        crate::registry::ReasoningEffort::Low;
    pub const DEFAULT_EFFORT_REASON: &'static str =
        "manually-created task uses the default execution depth";

    pub fn required_capabilities(&self) -> Vec<String> {
        if !self.required_capabilities.is_empty() {
            return crate::registry::normalize_capability_names(&self.required_capabilities);
        }

        match self.role.as_str() {
            "developer" => Self::DEFAULT_REQUIRED_CAPABILITIES
                .into_iter()
                .map(str::to_owned)
                .collect(),
            "reviewer" => vec!["review".into()],
            "architect" => vec!["architecture".into()],
            "researcher" => vec!["research".into()],
            _ => Vec::new(),
        }
    }

    pub fn risk_policy(&self) -> crate::protocol::TaskRiskPolicy {
        crate::protocol::TaskRiskPolicy::from_factors(&self.risk_factors)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Backlog,
    Ready,
    Active,
    Review,
    AcceptanceReady,
    RevisionRequired,
    Blocked,
    Done,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Backlog => "backlog",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Review => "review",
            Self::AcceptanceReady => "acceptance_ready",
            Self::RevisionRequired => "revision_required",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        };
        f.write_str(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(role: &str, required_capabilities: Vec<&str>) -> Task {
        Task {
            id: "T-0001".into(),
            title: "Test".into(),
            objective: "Test capability requirements".into(),
            role: role.into(),
            priority: TaskPriority::Normal,
            status: TaskStatus::Backlog,
            cancellation_reason: None,
            required_capabilities: required_capabilities
                .into_iter()
                .map(str::to_owned)
                .collect(),
            scope_mode: None,
            context_files: Vec::new(),
            expected_changes: Vec::new(),
            reasoning_effort: None,
            effort_reason: None,
            risk_factors: Vec::new(),
        }
    }

    #[test]
    fn developer_defaults_to_canonical_code_and_command_execution() {
        assert_eq!(
            task("developer", vec![]).required_capabilities(),
            vec!["code", "command_execution"]
        );
    }

    #[test]
    fn non_coding_roles_get_role_specific_defaults() {
        assert_eq!(
            task("architect", vec![]).required_capabilities(),
            vec!["architecture"]
        );
        assert_eq!(
            task("reviewer", vec![]).required_capabilities(),
            vec!["review"]
        );
        assert_eq!(
            task("researcher", vec![]).required_capabilities(),
            vec!["research"]
        );
    }

    #[test]
    fn explicit_capabilities_override_role_defaults() {
        assert_eq!(
            task("developer", vec!["review", "architecture"]).required_capabilities(),
            vec!["review", "architecture"]
        );
    }

    #[test]
    fn unknown_role_without_explicit_requirements_has_no_defaults() {
        assert!(task("custom", vec![]).required_capabilities().is_empty());
    }
}
