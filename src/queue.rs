use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::scheduler::{self, CandidateEvaluation, CandidateStatus, ScheduleDecision};
use crate::storage::{Database, DbError};
use crate::task::{Task, TaskStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueCategory {
    Ready,
    Blocked,
    Active,
    Review,
    Done,
    Cancelled,
    Backlog,
}

impl fmt::Display for QueueCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Ready => "READY",
            Self::Blocked => "BLOCKED",
            Self::Active => "ACTIVE",
            Self::Review => "REVIEW",
            Self::Done => "DONE",
            Self::Cancelled => "CANCELLED",
            Self::Backlog => "BACKLOG",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub task_id: String,
    pub status: Option<TaskStatus>,
    pub is_done: bool,
}

/// An explicit, machine-testable reason why a task cannot currently run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BlockingReason {
    DependencyBlocked {
        incomplete_dependencies: Vec<DependencyInfo>,
    },
    NoEligibleAgent {
        explanation: String,
        rejections: Vec<CandidateEvaluation>,
    },
    PersistedLifecycleBlocked,
}

impl fmt::Display for BlockingReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DependencyBlocked {
                incomplete_dependencies,
            } => {
                let dependencies = incomplete_dependencies
                    .iter()
                    .map(|dependency| match dependency.status {
                        Some(status) => format!("{} [{}]", dependency.task_id, status),
                        None => format!("{} [unknown]", dependency.task_id),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "incomplete dependencies: {dependencies}")
            }
            Self::NoEligibleAgent { explanation, .. } => {
                write!(f, "no eligible agent: {explanation}")
            }
            Self::PersistedLifecycleBlocked => {
                f.write_str("task is persistently lifecycle-blocked")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub task: Task,
    pub category: QueueCategory,
    pub dependencies: Vec<DependencyInfo>,
    pub waiting_on: Vec<String>,
    pub blocking_reasons: Vec<BlockingReason>,
    pub active_agent: Option<String>,
    pub recommended_agent: Option<String>,
    pub schedule_decision: Option<ScheduleDecision>,
    pub recommended_execution: Option<crate::execution::ExecutionResolution>,
}

/// Backwards-compatible name for Queue v1 callers.
pub type QueueItem = QueueEntry;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueReport {
    pub ready: Vec<QueueEntry>,
    pub blocked: Vec<QueueEntry>,
    pub active: Vec<QueueEntry>,
    pub review: Vec<QueueEntry>,
    pub done: Vec<QueueEntry>,
    pub cancelled: Vec<QueueEntry>,
    pub backlog: Vec<QueueEntry>,
}

impl QueueReport {
    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
            && self.blocked.is_empty()
            && self.active.is_empty()
            && self.review.is_empty()
            && self.done.is_empty()
            && self.cancelled.is_empty()
            && self.backlog.is_empty()
    }

    pub fn all_items(&self) -> Vec<&QueueEntry> {
        let mut items = Vec::new();
        items.extend(&self.cancelled);
        items.extend(&self.done);
        items.extend(&self.ready);
        items.extend(&self.blocked);
        items.extend(&self.backlog);
        items.extend(&self.review);
        items.extend(&self.active);
        items
    }

    pub fn find_item(&self, task_id: &str) -> Option<&QueueEntry> {
        self.all_items().into_iter().find(|i| i.task.id == task_id)
    }

    pub fn format_concise(&self) -> String {
        let mut sections = Vec::new();

        let categories = [
            ("CANCELLED", &self.cancelled),
            ("DONE", &self.done),
            ("READY", &self.ready),
            ("BLOCKED", &self.blocked),
            ("BACKLOG", &self.backlog),
            ("REVIEW", &self.review),
            ("ACTIVE", &self.active),
        ];
        for (name, items) in categories {
            if items.is_empty() {
                continue;
            }
            let mut s = format!("{name}\n");
            for item in items {
                let target = match name {
                    "ACTIVE" | "REVIEW" => item.active_agent.as_deref().unwrap_or(&item.task.title),
                    _ => &item.task.title,
                };
                s.push_str(&format!("{:<7} {}\n", item.task.id, target));
                if name == "BLOCKED" {
                    if !item.waiting_on.is_empty() {
                        s.push_str(&format!(
                            "        waiting on: {}\n",
                            item.waiting_on.join(", ")
                        ));
                    } else if !item.blocking_reasons.is_empty() {
                        let reasons = item
                            .blocking_reasons
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; ");
                        s.push_str(&format!("        blocked: {reasons}\n"));
                    }
                }
            }
            sections.push(s);
        }

        if sections.is_empty() {
            "No tasks in queue.\n".to_string()
        } else {
            sections.join("\n")
        }
    }

    pub fn format_explain(&self) -> String {
        let mut out = String::new();
        let categories = [
            (QueueCategory::Cancelled, &self.cancelled),
            (QueueCategory::Done, &self.done),
            (QueueCategory::Ready, &self.ready),
            (QueueCategory::Blocked, &self.blocked),
            (QueueCategory::Backlog, &self.backlog),
            (QueueCategory::Review, &self.review),
            (QueueCategory::Active, &self.active),
        ];

        let mut first_cat = true;
        for (cat, items) in categories {
            if items.is_empty() {
                continue;
            }
            if !first_cat {
                out.push('\n');
            }
            first_cat = false;
            out.push_str(&format!("=== {cat} ===\n"));

            for item in items {
                out.push_str(&format!("\n{} - {}\n", item.task.id, item.task.title));
                out.push_str(&format!("  Role:                 {}\n", item.task.role));
                let resolution =
                    item.recommended_execution
                        .as_ref()
                        .cloned()
                        .unwrap_or_else(|| {
                            crate::execution::resolve(&item.task.role, None, None, None, None)
                        });
                out.push_str(&format!(
                    "  Execution:            class={}, model={}, effort={}, source={}\n",
                    resolution.class.as_str(),
                    resolution.model.as_deref().unwrap_or("default"),
                    resolution
                        .reasoning_effort
                        .map(|effort| effort.as_str())
                        .unwrap_or("default"),
                    resolution.source,
                ));
                out.push_str(&format!("  Persisted Status:     {}\n", item.task.status));
                out.push_str(&format!(
                    "  Capabilities:         {}\n",
                    if item.task.required_capabilities().is_empty() {
                        "none".to_string()
                    } else {
                        item.task.required_capabilities().join(", ")
                    }
                ));

                if item.dependencies.is_empty() {
                    out.push_str("  Dependencies:         none\n");
                } else {
                    let dep_strs: Vec<String> = item
                        .dependencies
                        .iter()
                        .map(|d| {
                            let status_str = d
                                .status
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "unknown".to_string());
                            format!("{} [{}]", d.task_id, status_str)
                        })
                        .collect();
                    out.push_str(&format!(
                        "  Dependencies:         {}\n",
                        dep_strs.join(", ")
                    ));
                }

                if !item.blocking_reasons.is_empty() {
                    out.push_str(&format!(
                        "  Blocking reasons:     {}\n",
                        item.blocking_reasons
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                }

                if let Some(ref agent) = item.active_agent {
                    out.push_str(&format!("  Active agent:         {}\n", agent));
                }

                if let Some(ref agent) = item.recommended_agent {
                    out.push_str(&format!("  Recommended agent:    {}\n", agent));
                }

                if let Some(ref decision) = item.schedule_decision {
                    out.push_str("  Candidate Evaluations:\n");
                    for cand in &decision.candidates {
                        match &cand.status {
                            CandidateStatus::Eligible => {
                                let quota_str = cand
                                    .quota_remaining_percent
                                    .map(|q| format!("{q}%"))
                                    .unwrap_or_else(|| "unknown".to_string());
                                out.push_str(&format!(
                                    "    - {}: ELIGIBLE (mode: {}, priority: {}, quota: {})\n",
                                    cand.agent_id, cand.execution_mode, cand.priority, quota_str
                                ));
                            }
                            CandidateStatus::Rejected(reason) => {
                                out.push_str(&format!(
                                    "    - {}: REJECTED ({})\n",
                                    cand.agent_id,
                                    reason.description()
                                ));
                            }
                        }
                    }
                    out.push_str(&format!(
                        "  Scheduler explanation: {}\n",
                        decision.explanation
                    ));
                }
            }
        }

        if out.is_empty() {
            "No tasks in queue.\n".to_string()
        } else {
            out
        }
    }
}

pub fn compute_queue(db: &Database) -> Result<QueueReport, DbError> {
    let tasks = db.list_tasks()?;
    let agents = db.list_agents()?;
    let quota_reserve = db.quota_reserve()?;
    let busy_agents = db.list_busy_agents()?.into_iter().collect::<HashSet<_>>();
    let all_deps = db.list_all_dependencies()?;

    let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
    for (task_id, depends_on) in all_deps {
        deps_map.entry(task_id).or_default().push(depends_on);
    }

    let task_map: HashMap<String, Task> = tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();

    let mut report = QueueReport::default();

    for task in tasks {
        let dep_ids = deps_map.get(&task.id).cloned().unwrap_or_default();
        let mut dependencies = Vec::new();
        let mut incomplete_dependencies = Vec::new();

        for dep_id in &dep_ids {
            if let Some(dep_task) = task_map.get(dep_id) {
                let is_done = dep_task.status == TaskStatus::Done;
                let dependency = DependencyInfo {
                    task_id: dep_id.clone(),
                    status: Some(dep_task.status),
                    is_done,
                };
                if !is_done {
                    incomplete_dependencies.push(dependency.clone());
                }
                dependencies.push(dependency);
            } else {
                let dependency = DependencyInfo {
                    task_id: dep_id.clone(),
                    status: None,
                    is_done: false,
                };
                incomplete_dependencies.push(dependency.clone());
                dependencies.push(dependency);
            }
        }
        let waiting_on = incomplete_dependencies
            .iter()
            .map(|dependency| dependency.task_id.clone())
            .collect::<Vec<_>>();

        let active_agent = if matches!(task.status, TaskStatus::Active | TaskStatus::Review) {
            let runs = db.list_agent_runs_for_task(&task.id)?;
            runs.first().map(|r| r.agent.clone())
        } else {
            None
        };
        let persisted_execution = crate::execution::resolve_with_template(
            &task.role,
            &db.execution_template(crate::execution::class_for_role(&task.role))?,
            None,
            None,
            None,
            None,
        );

        match task.status {
            TaskStatus::Done => {
                report.done.push(QueueItem {
                    task,
                    category: QueueCategory::Done,
                    dependencies,
                    waiting_on,
                    blocking_reasons: Vec::new(),
                    active_agent: None,
                    recommended_agent: None,
                    schedule_decision: None,
                    recommended_execution: Some(persisted_execution.clone()),
                });
            }
            TaskStatus::Cancelled => {
                report.cancelled.push(QueueItem {
                    task,
                    category: QueueCategory::Cancelled,
                    dependencies,
                    waiting_on,
                    blocking_reasons: Vec::new(),
                    active_agent: None,
                    recommended_agent: None,
                    schedule_decision: None,
                    recommended_execution: Some(persisted_execution.clone()),
                });
            }
            TaskStatus::Review => {
                report.review.push(QueueItem {
                    task,
                    category: QueueCategory::Review,
                    dependencies,
                    waiting_on,
                    blocking_reasons: Vec::new(),
                    active_agent,
                    recommended_agent: None,
                    schedule_decision: None,
                    recommended_execution: Some(persisted_execution.clone()),
                });
            }
            TaskStatus::Active => {
                report.active.push(QueueItem {
                    task,
                    category: QueueCategory::Active,
                    dependencies,
                    waiting_on,
                    blocking_reasons: Vec::new(),
                    active_agent,
                    recommended_agent: None,
                    schedule_decision: None,
                    recommended_execution: Some(persisted_execution.clone()),
                });
            }
            TaskStatus::Blocked => {
                let mut blocking_reasons = vec![BlockingReason::PersistedLifecycleBlocked];
                if !incomplete_dependencies.is_empty() {
                    blocking_reasons.push(BlockingReason::DependencyBlocked {
                        incomplete_dependencies,
                    });
                }
                report.blocked.push(QueueItem {
                    task,
                    category: QueueCategory::Blocked,
                    dependencies,
                    waiting_on,
                    blocking_reasons,
                    active_agent: None,
                    recommended_agent: None,
                    schedule_decision: None,
                    recommended_execution: Some(persisted_execution.clone()),
                });
            }
            TaskStatus::Backlog | TaskStatus::Ready => {
                if !incomplete_dependencies.is_empty() {
                    let blocking_reasons = vec![BlockingReason::DependencyBlocked {
                        incomplete_dependencies,
                    }];
                    report.blocked.push(QueueItem {
                        task,
                        category: QueueCategory::Blocked,
                        dependencies,
                        waiting_on,
                        blocking_reasons,
                        active_agent: None,
                        recommended_agent: None,
                        schedule_decision: None,
                        recommended_execution: Some(persisted_execution.clone()),
                    });
                } else {
                    let decision = scheduler::schedule_with_busy_and_quota_reserve(
                        &task,
                        &agents,
                        None,
                        &busy_agents,
                        quota_reserve,
                    )
                    .map_err(|e| DbError::Scheduler(e.to_string()))?;

                    if let Some(ref selected) = decision.selected_agent_id {
                        let selected = selected.clone();
                        let template =
                            db.execution_template(crate::execution::class_for_role(&task.role))?;
                        let recommended_execution = agents
                            .iter()
                            .find(|agent| agent.id == selected)
                            .map(|agent| {
                                crate::execution::resolve_with_template(
                                    &task.role,
                                    &template,
                                    agent.model.as_deref(),
                                    agent.reasoning_effort,
                                    None,
                                    None,
                                )
                            });
                        report.ready.push(QueueItem {
                            task,
                            category: QueueCategory::Ready,
                            dependencies,
                            waiting_on: Vec::new(),
                            blocking_reasons: Vec::new(),
                            active_agent: None,
                            recommended_agent: Some(selected),
                            schedule_decision: Some(decision),
                            recommended_execution,
                        });
                    } else {
                        let blocking_reasons = vec![BlockingReason::NoEligibleAgent {
                            explanation: decision.explanation.clone(),
                            rejections: decision
                                .candidates
                                .iter()
                                .filter(|candidate| {
                                    matches!(candidate.status, CandidateStatus::Rejected(_))
                                })
                                .cloned()
                                .collect(),
                        }];
                        report.backlog.push(QueueItem {
                            task,
                            category: QueueCategory::Backlog,
                            dependencies,
                            waiting_on: Vec::new(),
                            blocking_reasons,
                            active_agent: None,
                            recommended_agent: None,
                            schedule_decision: Some(decision),
                            recommended_execution: Some(persisted_execution.clone()),
                        });
                    }
                }
            }
        }
    }

    Ok(report)
}
