use std::collections::HashMap;
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
    AcceptanceReady,
    RevisionRequired,
    Done,
    Cancelled,
    Backlog,
}

/// Return the queue category canonically implied by a task status and its
/// dependency blocking facts.
pub(crate) fn category_for_status(
    status: TaskStatus,
    has_incomplete_dependencies: bool,
) -> QueueCategory {
    match status {
        TaskStatus::Backlog | TaskStatus::Ready if has_incomplete_dependencies => {
            QueueCategory::Blocked
        }
        TaskStatus::Backlog => QueueCategory::Backlog,
        TaskStatus::Ready => QueueCategory::Ready,
        TaskStatus::Active => QueueCategory::Active,
        TaskStatus::Review => QueueCategory::Review,
        TaskStatus::AcceptanceReady => QueueCategory::AcceptanceReady,
        TaskStatus::RevisionRequired => QueueCategory::RevisionRequired,
        TaskStatus::Blocked => QueueCategory::Blocked,
        TaskStatus::Done => QueueCategory::Done,
        TaskStatus::Cancelled => QueueCategory::Cancelled,
    }
}

/// Check whether a queue category is one the canonical queue can expose for a
/// task status and its dependency facts. A Ready task without an eligible
/// agent remains in the backlog until it can be dispatched.
pub(crate) fn phase_is_compatible(
    status: TaskStatus,
    phase: QueueCategory,
    has_incomplete_dependencies: bool,
) -> bool {
    if has_incomplete_dependencies && matches!(status, TaskStatus::Backlog | TaskStatus::Ready) {
        return phase == QueueCategory::Blocked;
    }
    if phase == category_for_status(status, false) {
        return true;
    }
    status == TaskStatus::Ready && phase == QueueCategory::Backlog
}

impl fmt::Display for QueueCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Ready => "READY",
            Self::Blocked => "BLOCKED",
            Self::Active => "ACTIVE",
            Self::Review => "REVIEW",
            Self::AcceptanceReady => "ACCEPTANCE READY",
            Self::RevisionRequired => "REVISION REQUIRED",
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
    pub acceptance_ready: Vec<QueueEntry>,
    pub revision_required: Vec<QueueEntry>,
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
            && self.acceptance_ready.is_empty()
            && self.revision_required.is_empty()
            && self.done.is_empty()
            && self.cancelled.is_empty()
            && self.backlog.is_empty()
    }

    pub fn all_items(&self) -> Vec<&QueueEntry> {
        let mut items = Vec::new();
        items.extend(&self.cancelled);
        items.extend(&self.done);
        items.extend(&self.blocked);
        items.extend(&self.backlog);
        items.extend(&self.ready);
        items.extend(&self.review);
        items.extend(&self.acceptance_ready);
        items.extend(&self.revision_required);
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
            ("BLOCKED", &self.blocked),
            ("BACKLOG", &self.backlog),
            ("READY", &self.ready),
            ("REVIEW", &self.review),
            ("ACCEPTANCE READY", &self.acceptance_ready),
            ("REVISION REQUIRED", &self.revision_required),
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
            (QueueCategory::Blocked, &self.blocked),
            (QueueCategory::Backlog, &self.backlog),
            (QueueCategory::Ready, &self.ready),
            (QueueCategory::Review, &self.review),
            (QueueCategory::AcceptanceReady, &self.acceptance_ready),
            (QueueCategory::RevisionRequired, &self.revision_required),
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
                                out.push_str(&format!(
                                    "    - {}: ELIGIBLE (mode: {}, priority: {}, quota: {}, tier: {})\n",
                                    cand.agent_id,
                                    cand.execution_mode,
                                    cand.priority,
                                    cand.quota_observation.description(),
                                    cand.economy_tier.as_str()
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
    let all_deps = db.list_all_dependencies()?;
    let runs = match db.get_project_id()? {
        Some(project_id) => db.list_agent_runs(project_id, usize::MAX)?,
        None => Vec::new(),
    };
    compute_queue_from_facts(db, tasks, all_deps, &runs)
}

pub(crate) fn compute_queue_from_facts(
    db: &Database,
    tasks: Vec<Task>,
    all_deps: Vec<(String, String)>,
    runs: &[crate::storage::AgentRun],
) -> Result<QueueReport, DbError> {
    let mut deps_map: HashMap<String, Vec<String>> = HashMap::new();
    for (task_id, depends_on) in all_deps {
        deps_map.entry(task_id).or_default().push(depends_on);
    }

    let task_map: HashMap<String, Task> = tasks.iter().map(|t| (t.id.clone(), t.clone())).collect();
    let mut latest_agents = HashMap::new();
    for run in runs {
        if let Some(task_id) = &run.task_id {
            latest_agents
                .entry(task_id.clone())
                .or_insert_with(|| run.agent.clone());
        }
    }

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
            latest_agents.get(&task.id).cloned()
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
            TaskStatus::AcceptanceReady => {
                report.acceptance_ready.push(QueueItem {
                    task,
                    category: QueueCategory::AcceptanceReady,
                    dependencies,
                    waiting_on,
                    blocking_reasons: Vec::new(),
                    active_agent: None,
                    recommended_agent: None,
                    schedule_decision: None,
                    recommended_execution: Some(persisted_execution.clone()),
                });
            }
            TaskStatus::RevisionRequired => {
                report.revision_required.push(QueueItem {
                    task,
                    category: QueueCategory::RevisionRequired,
                    dependencies,
                    waiting_on,
                    blocking_reasons: Vec::new(),
                    active_agent: None,
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
                    let pending_escalation = db
                        .pending_escalation_request(&task.id)?
                        .map(|persisted| persisted.request);
                    let economy = scheduler::resolve_task_economy(
                        db,
                        &task,
                        crate::registry::AgentAction::Code,
                        scheduler::EconomyOverrides::default(),
                        None,
                        None,
                        None,
                        None,
                        scheduler::TransportEligibility::Strict,
                        pending_escalation,
                        "queue_explanation",
                    )
                    .map_err(|e| DbError::Scheduler(e.to_string()))?;
                    let decision = economy.schedule;

                    if let Some(ref selected) = decision.selected_agent_id {
                        let selected = selected.clone();
                        let recommended_execution =
                            economy.resolution.map(|resolution| resolution.execution);
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

/// Enforce the same task-level readiness used by the queue before dispatch.
/// Agent selection/validation remains the scheduler's responsibility.
pub fn ensure_dispatchable(db: &Database, task_id: &str) -> Result<(), DbError> {
    let report = compute_queue(db)?;
    let entry = report
        .find_item(task_id)
        .ok_or_else(|| DbError::TaskNotFound(task_id.to_string()))?;
    if entry.category != QueueCategory::Ready {
        return Err(DbError::Scheduler(format!(
            "task '{}' is not dispatchable: {}",
            task_id,
            entry
                .blocking_reasons
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{QueueCategory, phase_is_compatible};
    use crate::task::TaskStatus;

    #[test]
    fn phase_compatibility_respects_dependency_projection() {
        let cases = [
            (TaskStatus::Ready, QueueCategory::Blocked, true, true),
            (TaskStatus::Ready, QueueCategory::Ready, true, false),
            (TaskStatus::Ready, QueueCategory::Backlog, true, false),
            (TaskStatus::Backlog, QueueCategory::Blocked, true, true),
            (TaskStatus::Backlog, QueueCategory::Backlog, true, false),
            (TaskStatus::Ready, QueueCategory::Backlog, false, true),
            (TaskStatus::Active, QueueCategory::Active, true, true),
            (TaskStatus::Review, QueueCategory::Review, true, true),
            (
                TaskStatus::AcceptanceReady,
                QueueCategory::AcceptanceReady,
                true,
                true,
            ),
            (
                TaskStatus::RevisionRequired,
                QueueCategory::RevisionRequired,
                true,
                true,
            ),
            (TaskStatus::Done, QueueCategory::Done, true, true),
            (TaskStatus::Cancelled, QueueCategory::Cancelled, true, true),
        ];

        for (status, phase, has_incomplete_dependencies, expected) in cases {
            assert_eq!(
                phase_is_compatible(status, phase, has_incomplete_dependencies),
                expected,
                "status={status:?}, phase={phase:?}, incomplete_dependencies={has_incomplete_dependencies}"
            );
        }
    }

    #[test]
    fn dependency_free_ready_task_can_remain_in_backlog() {
        assert!(phase_is_compatible(
            TaskStatus::Ready,
            QueueCategory::Backlog,
            false
        ));
        assert!(phase_is_compatible(
            TaskStatus::Ready,
            QueueCategory::Ready,
            false
        ));
    }
}
