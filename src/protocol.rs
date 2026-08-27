use serde::{Deserialize, Serialize};

use crate::task::{Task, TaskPriority, TaskScopeMode};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectReport {
    pub protocol_version: u32,
    pub project: ReportProject,
    pub engineering_contract: String,
    pub architecture: ReportArchitecture,
    pub lifecycle: ReportLifecycle,
    pub agents: Vec<ReportAgent>,
    pub queue: crate::queue::QueueReport,
    pub recent_work: Vec<ReportRun>,
    pub risks: Vec<String>,
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub role_boundaries: Vec<String>,
    #[serde(default)]
    pub planning_constraints: Vec<String>,
    #[serde(default)]
    pub approval_requirements: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportProject {
    pub name: String,
    pub repository: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReportArchitecture {
    pub modules: Vec<String>,
    pub boundaries: Vec<String>,
    #[serde(default)]
    pub discovery: std::collections::BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportLifecycle {
    pub counts: std::collections::BTreeMap<String, usize>,
    pub tasks: Vec<TaskSummary>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportAgent {
    pub id: String,
    pub display_name: String,
    pub enabled: bool,
    pub status: String,
    pub execution_mode: String,
    pub capabilities: Vec<String>,
    pub busy: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReportRun {
    pub task_id: Option<String>,
    pub agent: String,
    pub status: String,
    pub output: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningRequest {
    pub protocol_version: u32,
    pub kind: String,
    pub project: Option<ReportProject>,
    pub engineering_contract: String,
    pub objective: String,
    pub constraints: Vec<String>,
    pub target_platforms: Vec<String>,
    pub stack: Vec<String>,
    pub non_goals: Vec<String>,
    pub deliverables: Vec<String>,
    pub definition_of_done: Vec<String>,
    pub response_schema: PlanResponseSchema,
    #[serde(default)]
    pub role_boundaries: Vec<String>,
    #[serde(default)]
    pub planning_constraints: Vec<String>,
    #[serde(default)]
    pub approval_requirements: Vec<String>,
    #[serde(default)]
    pub current_state: Option<PlanningProjectState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_report: Option<ProjectReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_snapshot: Option<crate::discovery::ProjectDiscoverySnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanningProjectState {
    pub task_counts: std::collections::BTreeMap<String, usize>,
    pub ready_tasks: Vec<TaskSummary>,
    pub active_tasks: Vec<TaskSummary>,
    pub review_tasks: Vec<TaskSummary>,
    pub blocked_tasks: Vec<TaskSummary>,
    pub usable_agents: Vec<String>,
    pub busy_agents: Vec<String>,
    pub quota_reserve_percent: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanResponseSchema {
    pub name: String,
    pub protocol_version: u32,
    pub fields: Vec<String>,
    pub task_fields: Vec<String>,
}

impl PlanResponseSchema {
    pub fn v1() -> Self {
        Self {
            name: "PlanResponse".into(),
            protocol_version: PROTOCOL_VERSION,
            fields: [
                "protocol_version",
                "objective",
                "assumptions",
                "risks",
                "questions",
                "tasks",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            task_fields: [
                "local_id",
                "title",
                "objective",
                "role",
                "priority",
                "capabilities",
                "scope_mode",
                "context_files",
                "expected_changes",
                "unchanged",
                "acceptance_criteria",
                "required_tests",
                "validation",
                "execution_hints",
                "depends_on",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanResponse {
    pub protocol_version: u32,
    pub objective: String,
    pub assumptions: Vec<String>,
    pub risks: Vec<String>,
    pub questions: Vec<String>,
    pub tasks: Vec<TaskProposal>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskProposal {
    pub local_id: String,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    #[serde(alias = "dependencies")]
    pub depends_on: Vec<String>,
    pub capabilities: Vec<String>,
    pub scope_mode: Option<TaskScopeMode>,
    pub context_files: Vec<String>,
    pub expected_changes: Vec<String>,
    pub unchanged: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    pub validation: Vec<String>,
    pub execution_hints: ExecutionHints,
}

/// Compatibility name for callers of the v1 planning API.
pub type PlannedTask = TaskProposal;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionHints {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl TaskProposal {
    pub fn validate(&self) -> anyhow::Result<()> {
        for (name, value) in [
            ("local_id", &self.local_id),
            ("title", &self.title),
            ("objective", &self.objective),
            ("role", &self.role),
        ] {
            if value.trim().is_empty() {
                anyhow::bail!("task proposal {name} must not be empty")
            }
        }
        for (name, values) in [
            ("expected_changes", &self.expected_changes),
            ("unchanged", &self.unchanged),
            ("acceptance_criteria", &self.acceptance_criteria),
            ("required_tests", &self.required_tests),
            ("validation", &self.validation),
        ] {
            if values.is_empty() {
                anyhow::bail!("task proposal '{}' must not be empty", self.local_id)
            }
            if values.iter().any(|value| value.trim().is_empty()) {
                anyhow::bail!("task proposal {name} contains an empty requirement")
            }
        }
        Ok(())
    }
}

impl PlanningRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            anyhow::bail!("unsupported planning protocol version")
        }
        if self.objective.trim().is_empty() {
            anyhow::bail!("planning objective must not be empty")
        }
        Ok(())
    }
}
impl PlanResponse {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != PROTOCOL_VERSION {
            anyhow::bail!("unsupported planning protocol version")
        }
        let ids: std::collections::HashSet<_> =
            self.tasks.iter().map(|t| t.local_id.as_str()).collect();
        if ids.len() != self.tasks.len() {
            anyhow::bail!("plan task IDs must be unique")
        }
        for task in &self.tasks {
            task.validate()?;
            if matches!(
                task.scope_mode,
                Some(TaskScopeMode::Focused | TaskScopeMode::Module)
            ) && task.context_files.is_empty()
            {
                anyhow::bail!(
                    "plan task '{}' with targeted scope must list at least one context file",
                    task.local_id
                )
            }
            for (field, paths) in [
                ("context_files", &task.context_files),
                ("expected_changes", &task.expected_changes),
            ] {
                for path in paths {
                    if std::path::Path::new(path).is_absolute()
                        || path.starts_with('/')
                        || path.starts_with('\\')
                        || (path.as_bytes().get(1) == Some(&b':')
                            && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic))
                        || path.split(['/', '\\']).any(|segment| segment == "..")
                    {
                        anyhow::bail!(
                            "plan task '{}' has invalid {} path '{}': absolute and '..' paths are not allowed",
                            task.local_id,
                            field,
                            path
                        )
                    }
                }
            }
            for dependency in &task.depends_on {
                if dependency == &task.local_id || !ids.contains(dependency.as_str()) {
                    anyhow::bail!("plan dependency '{}' is not a task in the plan", dependency)
                }
            }
        }
        fn visit(
            id: &str,
            tasks: &std::collections::HashMap<&str, &TaskProposal>,
            visiting: &mut std::collections::HashSet<String>,
            visited: &mut std::collections::HashSet<String>,
        ) -> bool {
            if visiting.contains(id) {
                return true;
            }
            if visited.contains(id) {
                return false;
            }
            visiting.insert(id.to_owned());
            for dependency in &tasks[id].depends_on {
                if visit(dependency, tasks, visiting, visited) {
                    return true;
                }
            }
            visiting.remove(id);
            visited.insert(id.to_owned());
            false
        }
        let tasks = self
            .tasks
            .iter()
            .map(|task| (task.local_id.as_str(), task))
            .collect();
        let mut visiting = std::collections::HashSet::new();
        let mut visited = std::collections::HashSet::new();
        if self
            .tasks
            .iter()
            .any(|task| visit(&task.local_id, &tasks, &mut visiting, &mut visited))
        {
            anyhow::bail!("plan dependencies contain a cycle")
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> TaskProposal {
        TaskProposal {
            local_id: "one".into(),
            title: "One behavior".into(),
            objective: "Do one thing".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            depends_on: vec![],
            capabilities: vec!["code".into()],
            scope_mode: Some(TaskScopeMode::Focused),
            context_files: vec!["src/lib.rs".into()],
            expected_changes: vec!["src/lib.rs".into()],
            unchanged: vec!["CLI behavior".into()],
            acceptance_criteria: vec!["works".into()],
            required_tests: vec!["production test".into()],
            validation: vec!["cargo test".into()],
            execution_hints: ExecutionHints {
                class: Some("code".into()),
                model: Some("x".into()),
                effort: Some("low".into()),
            },
        }
    }

    #[test]
    fn task_proposal_round_trips_and_validates() {
        let value = proposal();
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<TaskProposal>(&json).unwrap(), value);
        assert!(value.validate().is_ok());
    }

    #[test]
    fn incomplete_task_proposals_are_rejected() {
        let mut value = proposal();
        value.acceptance_criteria.clear();
        assert!(value.validate().is_err());
    }

    #[test]
    fn task_proposal_missing_contract_field_is_rejected_during_deserialization() {
        let mut json = serde_json::to_value(proposal()).unwrap();
        json.as_object_mut().unwrap().remove("acceptance_criteria");
        assert!(serde_json::from_value::<TaskProposal>(json).is_err());
    }

    #[test]
    fn task_proposal_without_expected_changes_is_rejected() {
        let mut value = proposal();
        value.expected_changes.clear();
        assert!(value.validate().is_err());
    }
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
