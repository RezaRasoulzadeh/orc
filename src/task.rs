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
    pub const DEFAULT_REQUIRED_CAPABILITIES: [&'static str; 2] = ["code", "terminal"];

    pub fn required_capabilities(&self) -> Vec<String> {
        if !self.required_capabilities.is_empty() {
            return self.required_capabilities.clone();
        }

        match self.role.as_str() {
            "developer" => vec!["code".into(), "terminal".into()],
            "reviewer" => vec!["review".into()],
            "architect" => vec!["architecture".into()],
            "researcher" => vec!["research".into()],
            _ => Vec::new(),
        }
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
        }
    }

    #[test]
    fn developer_defaults_to_code_and_terminal() {
        assert_eq!(
            task("developer", vec![]).required_capabilities(),
            vec!["code", "terminal"]
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
