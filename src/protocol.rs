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
                "depends_on",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanResponse {
    pub protocol_version: u32,
    pub objective: String,
    pub assumptions: Vec<String>,
    pub risks: Vec<String>,
    pub questions: Vec<String>,
    pub tasks: Vec<PlannedTask>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedTask {
    pub local_id: String,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    #[serde(default)]
    #[serde(alias = "dependencies")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub scope_mode: Option<TaskScopeMode>,
    #[serde(default)]
    pub context_files: Vec<String>,
    #[serde(default)]
    pub expected_changes: Vec<String>,
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
            if task.local_id.trim().is_empty()
                || task.title.trim().is_empty()
                || task.objective.trim().is_empty()
                || task.role.trim().is_empty()
            {
                anyhow::bail!("plan tasks require local_id, title, objective, and role")
            }
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
            tasks: &std::collections::HashMap<&str, &PlannedTask>,
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
