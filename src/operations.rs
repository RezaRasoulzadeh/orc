use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::queue::{QueueCategory, QueueEntry, QueueReport};
use crate::registry::{EconomyTier, EscalationTrigger, ReasoningEffort};
use crate::storage::db::{
    LifecycleEvent, PersistedEscalationRequest, ProjectChangeEvidence, ProjectProviderInvocation,
    ProjectWorktreeMetadata, ReviewBlockerRecord,
};
use crate::storage::{AgentRun, Database, WorkerResult};
use crate::task::{Task, TaskContract, TaskPriority, TaskStatus};
use crate::validation::{ValidationFailureClassification, ValidationReport};

/// The provider-independent application/read boundary for persisted project
/// operations. It composes durable storage facts into stable operator views;
/// it does not mutate lifecycle, refresh quota, or invoke providers.
pub struct ProjectOperations<'a> {
    db: &'a Database,
    repository_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationalNextStep {
    Dispatch,
    WaitForExecution,
    RunSemanticReview,
    Revise,
    Accept,
    ResolveBlocker,
    SatisfyDependencies,
    ConfigureEligibleAgent,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationState {
    None,
    Running,
    Passing,
    Failing,
    InfrastructureFailure,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockerState {
    New,
    Unresolved,
    Regressed,
    Resolved,
    Unknown,
}

impl BlockerState {
    pub fn is_actionable(self) -> bool {
        matches!(self, Self::New | Self::Unresolved | Self::Regressed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCommandSummary {
    pub command: String,
    pub passed: Option<bool>,
    pub failure_classification: Option<ValidationFailureClassification>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationSummary {
    pub state: ValidationState,
    pub recorded_state: Option<ValidationState>,
    pub run_id: Option<i64>,
    pub timestamp: Option<String>,
    pub latest_passing_run_id: Option<i64>,
    pub latest_passing_timestamp: Option<String>,
    pub is_current: Option<bool>,
    pub worktree_fingerprint: Option<String>,
    pub selected_commands: Vec<ValidationCommandSummary>,
    pub failure_classification: Option<ValidationFailureClassification>,
}

impl Default for ValidationSummary {
    fn default() -> Self {
        Self {
            state: ValidationState::None,
            recorded_state: None,
            run_id: None,
            timestamp: None,
            latest_passing_run_id: None,
            latest_passing_timestamp: None,
            is_current: None,
            worktree_fingerprint: None,
            selected_commands: Vec::new(),
            failure_classification: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockerSummary {
    pub id: String,
    pub key: String,
    pub state: BlockerState,
    pub actionable: bool,
    pub summary: String,
    pub requirement: String,
    pub evidence: String,
    pub severity: String,
    pub acceptance_condition: String,
    pub originating_review_run_id: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewOperationsSummary {
    pub run_id: Option<i64>,
    pub verdict: Option<String>,
    pub timestamp: Option<String>,
    pub applies_to_current_change: Option<bool>,
    pub ready_for_review: bool,
    pub actionable_blockers: usize,
    pub unresolved_blockers: usize,
    pub regressed_blockers: usize,
    pub resolved_blockers: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub total_tokens: Option<i64>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub uncached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_ratio: Option<f64>,
    pub observations_with_usage: usize,
    pub observations_without_usage: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaObservationSummary {
    pub remaining_percent: Option<i64>,
    pub reset_at: Option<String>,
    pub checked_at: Option<String>,
    pub source: Option<String>,
    pub freshness: Option<String>,
    pub reserve_percent: Option<i64>,
    pub refresh_supported: Option<bool>,
    pub refresh_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyResolutionSummary {
    pub invocation_id: i64,
    pub run_id: i64,
    pub task_id: Option<String>,
    pub purpose: String,
    pub action: Option<String>,
    pub attempt: usize,
    pub timestamp: String,
    pub finished_at: Option<String>,
    pub outcome: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
    pub tier: EconomyTier,
    pub source: Option<String>,
    pub selection_reason: Option<String>,
    pub selection_explanation: Option<String>,
    pub operator_override: bool,
    pub escalation_reason: Option<String>,
    pub quota: Option<QuotaObservationSummary>,
    pub legacy_missing_resolution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EscalationSummary {
    pub request_id: i64,
    pub task_id: String,
    pub trigger: EscalationTrigger,
    pub reason: String,
    pub previous_provider_invocation_id: i64,
    pub previous_tier: EconomyTier,
    pub previous_model: Option<String>,
    pub previous_effort: Option<ReasoningEffort>,
    pub previous_attempt: usize,
    pub requested_minimum_tier: EconomyTier,
    pub policy_attempt: usize,
    pub state: String,
    pub created_at: String,
    pub resulting_invocation_id: Option<i64>,
    pub resulting_tier: Option<EconomyTier>,
    pub resulting_model: Option<String>,
    pub resulting_effort: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionSummary {
    pub id: i64,
    pub task_id: Option<String>,
    pub agent: String,
    pub execution_mode: String,
    pub execution_class: String,
    pub status: String,
    pub phase: Option<String>,
    pub is_active: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub last_activity: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub outcome: Option<String>,
    pub failure_category: Option<String>,
    pub duration_ms: Option<i64>,
    pub persisted_model: Option<String>,
    pub persisted_effort: Option<ReasoningEffort>,
    pub persisted_resolution_source: String,
    pub latest_resolution: Option<EconomyResolutionSummary>,
    pub token_usage: TokenUsageSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOperationsSummary {
    pub task_id: String,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub priority: TaskPriority,
    pub lifecycle: TaskStatus,
    pub phase: QueueCategory,
    pub next_step: OperationalNextStep,
    pub cancellation_reason: Option<String>,
    pub current_run: Option<ExecutionSummary>,
    pub latest_run: Option<ExecutionSummary>,
    pub validation: ValidationSummary,
    pub review: ReviewOperationsSummary,
    pub actionable_blocker_count: usize,
    pub latest_resolution: Option<EconomyResolutionSummary>,
    pub token_usage: TokenUsageSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOperationsDetail {
    pub summary: TaskOperationsSummary,
    pub task: Task,
    pub contract: TaskContract,
    pub execution_condition: Option<ExecutionConditionSummary>,
    pub queue: Option<QueueEntry>,
    /// Newest run first; ties are broken by persisted run id descending.
    pub executions: Vec<ExecutionSummary>,
    /// Provider invocation order, oldest first.
    pub resolutions: Vec<EconomyResolutionSummary>,
    /// Escalation request order, oldest first.
    pub escalations: Vec<EscalationSummary>,
    /// Actionable blockers first, then stable first-seen/id order.
    pub blockers: Vec<BlockerSummary>,
    pub activity: Vec<OperationalEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionConditionSummary {
    pub kind: String,
    pub details: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationalEvent {
    pub id: i64,
    pub timestamp: String,
    pub kind: String,
    pub run_id: Option<i64>,
    pub agent_id: Option<String>,
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEconomySummary {
    pub task_id: String,
    pub lifecycle: TaskStatus,
    pub accepted: bool,
    pub invocation_count: usize,
    pub invocations_by_tier: BTreeMap<EconomyTier, usize>,
    pub escalation_count: usize,
    pub latest_resolution: Option<EconomyResolutionSummary>,
    pub token_usage: TokenUsageSummary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectEconomySummary {
    pub invocation_count: usize,
    pub invocations_by_tier: BTreeMap<EconomyTier, usize>,
    pub invocations_by_action: BTreeMap<String, usize>,
    pub escalation_count: usize,
    pub token_usage: TokenUsageSummary,
    pub accepted_tasks: usize,
    pub accepted_tasks_with_complete_token_usage: usize,
    pub accepted_tasks_by_tier: BTreeMap<EconomyTier, usize>,
    pub accepted_token_usage: TokenUsageSummary,
    pub tokens_per_accepted_task: Option<f64>,
    pub tasks: Vec<TaskEconomySummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectOperationsSnapshot {
    pub queue: QueueReport,
    pub tasks: Vec<TaskOperationsSummary>,
    pub economy: ProjectEconomySummary,
}

struct ProjectFacts {
    tasks: Vec<Task>,
    queue: QueueReport,
    runs: Vec<AgentRun>,
    results: HashMap<i64, WorkerResult>,
    invocations: Vec<ProjectProviderInvocation>,
    escalations: Vec<PersistedEscalationRequest>,
    blockers: Vec<ReviewBlockerRecord>,
    events: Vec<LifecycleEvent>,
    changes: HashMap<i64, ProjectChangeEvidence>,
    worktrees: Vec<ProjectWorktreeMetadata>,
}

impl<'a> ProjectOperations<'a> {
    pub fn new(db: &'a Database, repository_path: impl AsRef<Path>) -> Self {
        Self {
            db,
            repository_path: repository_path.as_ref().to_path_buf(),
        }
    }

    pub fn project_queue(&self) -> Result<QueueReport> {
        Ok(crate::queue::compute_queue(self.db)?)
    }

    pub fn project_name(&self) -> Result<Option<String>> {
        Ok(self.db.get_project_name()?)
    }

    pub fn tasks(&self) -> Result<Vec<Task>> {
        // This compatibility read remains valid before the first project is
        // initialized. Project-scoped operational summaries intentionally
        // require a persisted project identity.
        Ok(self.db.list_tasks()?)
    }

    pub fn task_summaries(&self) -> Result<Vec<TaskOperationsSummary>> {
        let facts = self.load_facts()?;
        facts
            .tasks
            .iter()
            .map(|task| self.task_summary_from_facts(task, &facts))
            .collect()
    }

    pub fn snapshot(&self) -> Result<ProjectOperationsSnapshot> {
        let facts = self.load_facts()?;
        let tasks = facts
            .tasks
            .iter()
            .map(|task| self.task_summary_from_facts(task, &facts))
            .collect::<Result<Vec<_>>>()?;
        Ok(ProjectOperationsSnapshot {
            queue: facts.queue.clone(),
            tasks,
            economy: self.economy_summary_from_facts(&facts),
        })
    }

    pub fn task_summary(&self, task_id: &str) -> Result<Option<TaskOperationsSummary>> {
        let facts = self.load_facts()?;
        facts
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| self.task_summary_from_facts(task, &facts))
            .transpose()
    }

    pub fn task_detail(&self, task_id: &str) -> Result<Option<TaskOperationsDetail>> {
        let facts = self.load_facts()?;
        let Some(task) = facts.tasks.iter().find(|task| task.id == task_id) else {
            return Ok(None);
        };
        let summary = self.task_summary_from_facts(task, &facts)?;
        let runs = runs_for_task(&facts, task_id);
        let executions = runs
            .iter()
            .map(|run| self.execution_summary_from_facts(run, &facts))
            .collect::<Vec<_>>();
        let resolutions = facts
            .invocations
            .iter()
            .filter(|item| item.task_id.as_deref() == Some(task_id))
            .map(economy_resolution_summary)
            .collect();
        let escalations = facts
            .escalations
            .iter()
            .filter(|item| item.task_id == task_id)
            .map(|item| escalation_summary(item, &facts.invocations))
            .collect();
        let mut blockers = facts
            .blockers
            .iter()
            .filter(|item| item.task_id == task_id)
            .map(blocker_summary)
            .collect::<Vec<_>>();
        blockers.sort_by(|left, right| {
            right
                .actionable
                .cmp(&left.actionable)
                .then_with(|| left.first_seen.cmp(&right.first_seen))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut activity = facts
            .events
            .iter()
            .filter(|event| event.task_id.as_deref() == Some(task_id))
            .map(operational_event)
            .collect::<Vec<_>>();
        activity.sort_by_key(|event| event.id);
        Ok(Some(TaskOperationsDetail {
            summary,
            task: task.clone(),
            contract: self
                .db
                .get_task_contract(task_id)?
                .unwrap_or_else(|| TaskContract::defaults(&task.objective)),
            execution_condition: self.db.get_task_execution_condition(task_id)?.map(|value| {
                ExecutionConditionSummary {
                    kind: value.kind,
                    details: value.details,
                    created_at: value.created_at,
                }
            }),
            queue: facts.queue.find_item(task_id).cloned(),
            executions,
            resolutions,
            escalations,
            blockers,
            activity,
        }))
    }

    pub fn execution_detail(&self, run_id: i64) -> Result<Option<ExecutionSummary>> {
        let facts = self.load_facts()?;
        Ok(facts
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .map(|run| self.execution_summary_from_facts(run, &facts)))
    }

    pub fn execution_summaries(
        &self,
        task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ExecutionSummary>> {
        let facts = self.load_facts()?;
        Ok(facts
            .runs
            .iter()
            .filter(|run| task_id.is_none_or(|id| run.task_id.as_deref() == Some(id)))
            .take(limit)
            .map(|run| self.execution_summary_from_facts(run, &facts))
            .collect())
    }

    pub fn economy_summary(&self) -> Result<ProjectEconomySummary> {
        let facts = self.load_facts()?;
        Ok(self.economy_summary_from_facts(&facts))
    }

    fn economy_summary_from_facts(&self, facts: &ProjectFacts) -> ProjectEconomySummary {
        let project_runs = facts.runs.iter().collect::<Vec<_>>();
        let token_usage = token_usage_for_runs(&project_runs, facts);
        let mut invocations_by_tier = BTreeMap::new();
        let mut invocations_by_action = BTreeMap::new();
        for item in &facts.invocations {
            let resolution = economy_resolution_summary(item);
            *invocations_by_tier.entry(resolution.tier).or_insert(0) += 1;
            *invocations_by_action
                .entry(
                    resolution
                        .action
                        .clone()
                        .unwrap_or_else(|| resolution.purpose.clone()),
                )
                .or_insert(0) += 1;
        }
        let mut accepted_tasks_by_tier = BTreeMap::new();
        let mut accepted_usage = TokenUsageAccumulator::default();
        let mut accepted_tasks = 0;
        let mut accepted_complete = 0;
        let mut task_metrics = Vec::new();
        for task in &facts.tasks {
            let runs = runs_for_task(facts, &task.id);
            let usage = token_usage_for_runs(&runs, facts);
            let invocations = facts
                .invocations
                .iter()
                .filter(|item| item.task_id.as_deref() == Some(task.id.as_str()))
                .collect::<Vec<_>>();
            let mut tiers = BTreeMap::new();
            for invocation in &invocations {
                *tiers
                    .entry(economy_resolution_summary(invocation).tier)
                    .or_insert(0) += 1;
            }
            let latest_resolution = invocations
                .last()
                .map(|item| economy_resolution_summary(item));
            let accepted = task.status == TaskStatus::Done;
            if accepted {
                accepted_tasks += 1;
                let tier = latest_resolution
                    .as_ref()
                    .map_or(EconomyTier::Unknown, |resolution| resolution.tier);
                *accepted_tasks_by_tier.entry(tier).or_insert(0) += 1;
                if usage.total_tokens.is_some() && usage.observations_without_usage == 0 {
                    accepted_complete += 1;
                }
                accepted_usage.add_summary(&usage);
            }
            task_metrics.push(TaskEconomySummary {
                task_id: task.id.clone(),
                lifecycle: task.status,
                accepted,
                invocation_count: invocations.len(),
                invocations_by_tier: tiers,
                escalation_count: invocations
                    .iter()
                    .filter(|item| item.invocation.escalation.is_some())
                    .count(),
                latest_resolution,
                token_usage: usage,
            });
        }
        let accepted_token_usage = accepted_usage.finish();
        let tokens_per_accepted_task = (accepted_tasks > 0 && accepted_complete == accepted_tasks)
            .then(|| {
                accepted_token_usage
                    .total_tokens
                    .map(|tokens| tokens as f64 / accepted_tasks as f64)
            })
            .flatten();
        ProjectEconomySummary {
            invocation_count: facts.invocations.len(),
            invocations_by_tier,
            invocations_by_action,
            escalation_count: facts.escalations.len(),
            token_usage,
            accepted_tasks,
            accepted_tasks_with_complete_token_usage: accepted_complete,
            accepted_tasks_by_tier,
            accepted_token_usage,
            tokens_per_accepted_task,
            tasks: task_metrics,
        }
    }

    fn load_facts(&self) -> Result<ProjectFacts> {
        let project_id = self
            .db
            .get_project_id()?
            .context("no project found in DB")?;
        let mut runs = self.db.list_agent_runs(project_id, usize::MAX)?;
        runs.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        let tasks = self.db.list_tasks_for_project(project_id)?;
        let queue = crate::queue::compute_queue_from_facts(
            self.db,
            tasks.clone(),
            self.db.list_all_dependencies()?,
            &runs,
        )?;
        Ok(ProjectFacts {
            tasks,
            queue,
            runs,
            results: self
                .db
                .project_worker_results(project_id)?
                .into_iter()
                .map(|result| (result.run_id, result))
                .collect(),
            invocations: self.db.project_provider_invocations(project_id)?,
            escalations: self.db.project_escalation_requests(project_id)?,
            blockers: self.db.project_review_blocker_ledger(project_id)?,
            events: self.db.list_lifecycle_events(usize::MAX)?,
            changes: self
                .db
                .project_change_evidence(project_id)?
                .into_iter()
                .map(|evidence| (evidence.run_id, evidence))
                .collect(),
            worktrees: self.db.project_worktree_metadata(project_id)?,
        })
    }

    fn task_summary_from_facts(
        &self,
        task: &Task,
        facts: &ProjectFacts,
    ) -> Result<TaskOperationsSummary> {
        let runs = runs_for_task(facts, &task.id);
        let latest_run = runs.first().copied();
        let current_run = runs.iter().copied().find(|run| is_active_run(&run.status));
        let validation = self.validation_summary(task, &runs, facts);
        let task_blockers = facts
            .blockers
            .iter()
            .filter(|blocker| blocker.task_id == task.id)
            .collect::<Vec<_>>();
        let review = self.review_summary(task, &runs, &task_blockers, &validation, facts);
        let queue = facts.queue.find_item(&task.id);
        let phase = queue
            .map(|entry| entry.category)
            .unwrap_or_else(|| queue_category_for_status(task.status));
        let resolutions = facts
            .invocations
            .iter()
            .filter(|item| item.task_id.as_deref() == Some(task.id.as_str()))
            .collect::<Vec<_>>();
        Ok(TaskOperationsSummary {
            task_id: task.id.clone(),
            title: task.title.clone(),
            objective: task.objective.clone(),
            role: task.role.clone(),
            priority: task.priority,
            lifecycle: task.status,
            phase,
            next_step: next_step(phase, queue),
            cancellation_reason: task.cancellation_reason.clone(),
            current_run: current_run.map(|run| self.execution_summary_from_facts(run, facts)),
            latest_run: latest_run.map(|run| self.execution_summary_from_facts(run, facts)),
            validation,
            review,
            actionable_blocker_count: task_blockers
                .iter()
                .filter(|blocker| blocker_state(&blocker.status).is_actionable())
                .count(),
            latest_resolution: resolutions
                .last()
                .map(|item| economy_resolution_summary(item)),
            token_usage: token_usage_for_runs(&runs, facts),
        })
    }

    fn execution_summary_from_facts(
        &self,
        run: &AgentRun,
        facts: &ProjectFacts,
    ) -> ExecutionSummary {
        let invocations = facts
            .invocations
            .iter()
            .filter(|item| item.invocation.parent_run_id == run.id)
            .collect::<Vec<_>>();
        let result = facts.results.get(&run.id);
        ExecutionSummary {
            id: run.id,
            task_id: run.task_id.clone(),
            agent: run.agent.clone(),
            execution_mode: run.execution_mode.clone(),
            execution_class: run.execution_class.clone(),
            status: run.status.clone(),
            phase: run.phase.clone(),
            is_active: is_active_run(&run.status),
            started_at: run.started_at.clone(),
            finished_at: run.finished_at.clone(),
            last_activity: run.last_activity.clone(),
            output: run.output.clone(),
            error: run.error.clone(),
            outcome: result.map(|value| value.outcome.clone()),
            failure_category: result.and_then(|value| value.failure_category.clone()),
            duration_ms: result.and_then(|value| value.duration_ms),
            persisted_model: run.resolved_model.clone(),
            persisted_effort: run.resolved_reasoning_effort,
            persisted_resolution_source: run.resolution_source.clone(),
            latest_resolution: invocations
                .last()
                .map(|item| economy_resolution_summary(item)),
            token_usage: token_usage_for_runs(&[run], facts),
        }
    }

    fn validation_summary(
        &self,
        task: &Task,
        runs: &[&AgentRun],
        facts: &ProjectFacts,
    ) -> ValidationSummary {
        let Some(run) = runs
            .iter()
            .copied()
            .filter(|run| run.execution_class != "review")
            .max_by_key(|run| run.id)
        else {
            return ValidationSummary::default();
        };
        let latest_passing = facts
            .events
            .iter()
            .filter(|event| {
                event.kind == "validation_result"
                    && event.run_id.is_some_and(|event_run| {
                        runs.iter().any(|candidate| {
                            candidate.id == event_run && candidate.execution_class != "review"
                        })
                    })
                    && event
                        .payload
                        .as_deref()
                        .and_then(|payload| serde_json::from_str::<ValidationReport>(payload).ok())
                        .is_some_and(|report| report.is_success())
            })
            .max_by_key(|event| event.id);
        let result_event = latest_event(facts, run.id, "validation_result");
        let selection_event = latest_event(facts, run.id, "validation_selection");
        let Some(event) = result_event else {
            return ValidationSummary {
                state: if is_active_run(&run.status)
                    && run
                        .phase
                        .as_deref()
                        .is_some_and(|phase| phase.contains("validation"))
                {
                    ValidationState::Running
                } else {
                    ValidationState::None
                },
                run_id: Some(run.id),
                latest_passing_run_id: latest_passing.and_then(|event| event.run_id),
                latest_passing_timestamp: latest_passing.map(|event| event.timestamp.clone()),
                ..ValidationSummary::default()
            };
        };
        let report = event
            .payload
            .as_deref()
            .and_then(|payload| serde_json::from_str::<ValidationReport>(payload).ok());
        let recorded_state = match report.as_ref() {
            Some(report) if report.is_success() => ValidationState::Passing,
            Some(report) if report.is_infrastructure_failure() => {
                ValidationState::InfrastructureFailure
            }
            Some(_) => ValidationState::Failing,
            None => ValidationState::InfrastructureFailure,
        };
        let selection = selection_event
            .and_then(|item| item.payload.as_deref())
            .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok());
        let fingerprint = selection
            .as_ref()
            .and_then(|value| value["worktree_fingerprint"].as_str())
            .map(str::to_owned);
        let current_fingerprint = self.current_fingerprint(task, run, facts);
        let is_current = match (fingerprint.as_deref(), current_fingerprint.as_deref()) {
            (Some(persisted), Some(current)) => Some(persisted == current),
            (_, _) if task.status == TaskStatus::Done => Some(true),
            _ => None,
        };
        let state = if is_current == Some(false) {
            ValidationState::Stale
        } else {
            recorded_state
        };
        let selected_names = selection
            .as_ref()
            .and_then(|value| value["selected_commands"].as_array())
            .map(|commands| {
                commands
                    .iter()
                    .filter_map(|command| command.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let selected_commands: Vec<ValidationCommandSummary> = if selected_names.is_empty() {
            report
                .as_ref()
                .map(|report| {
                    report
                        .steps
                        .iter()
                        .map(|step| ValidationCommandSummary {
                            command: step.command.clone(),
                            passed: Some(step.passed),
                            failure_classification: step.failure_classification,
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            selected_names
                .into_iter()
                .map(|command| {
                    let step = report.as_ref().and_then(|report| {
                        report.steps.iter().find(|step| step.command == command)
                    });
                    ValidationCommandSummary {
                        command,
                        passed: step.map(|step| step.passed),
                        failure_classification: step.and_then(|step| step.failure_classification),
                    }
                })
                .collect()
        };
        let failure_classification = report.as_ref().and_then(|report| {
            report
                .steps
                .iter()
                .find(|step| !step.passed)
                .and_then(|step| step.failure_classification)
                .or_else(|| {
                    report
                        .is_infrastructure_failure()
                        .then_some(ValidationFailureClassification::Infrastructure)
                })
        });
        ValidationSummary {
            state,
            recorded_state: Some(recorded_state),
            run_id: Some(run.id),
            timestamp: Some(event.timestamp.clone()),
            latest_passing_run_id: latest_passing.and_then(|event| event.run_id),
            latest_passing_timestamp: latest_passing.map(|event| event.timestamp.clone()),
            is_current,
            worktree_fingerprint: fingerprint,
            selected_commands,
            failure_classification,
        }
    }

    fn current_fingerprint(
        &self,
        task: &Task,
        run: &AgentRun,
        facts: &ProjectFacts,
    ) -> Option<String> {
        let metadata = facts
            .worktrees
            .iter()
            .filter(|item| item.task_id == task.id)
            .max_by(|left, right| {
                left.created_at
                    .cmp(&right.created_at)
                    .then_with(|| left.run_id.cmp(&right.run_id))
            });
        if let Some(metadata) = metadata {
            let path = self.repository_path.join(&metadata.worktree_path);
            if path.exists()
                && let Ok(changes) = crate::git::inspect_worktree(&path, &self.repository_path)
            {
                return Some(crate::automated::revision_worktree_fingerprint(&changes));
            }
        }
        facts
            .changes
            .get(&run.id)
            .map(|evidence| crate::automated::revision_worktree_fingerprint(&evidence.changes))
    }

    fn review_summary(
        &self,
        task: &Task,
        runs: &[&AgentRun],
        blockers: &[&ReviewBlockerRecord],
        validation: &ValidationSummary,
        facts: &ProjectFacts,
    ) -> ReviewOperationsSummary {
        let latest_review = runs
            .iter()
            .copied()
            .filter(|run| run.execution_class == "review")
            .max_by_key(|run| run.id);
        let latest_implementation = runs
            .iter()
            .copied()
            .filter(|run| run.execution_class != "review")
            .max_by_key(|run| run.id);
        let verdict = latest_review.and_then(|run| {
            run.output
                .as_deref()
                .and_then(|output| {
                    serde_json::from_str::<crate::automated::ReviewResult>(output).ok()
                })
                .map(|review| review.verdict)
        });
        let applies_to_current_change = latest_review.map(|review| {
            if latest_implementation.is_some_and(|implementation| implementation.id > review.id) {
                return false;
            }
            if task.status == TaskStatus::Done {
                return true;
            }
            let Some(reviewed) = facts.changes.get(&review.id) else {
                return false;
            };
            let metadata = facts
                .worktrees
                .iter()
                .filter(|item| item.task_id == task.id)
                .max_by(|left, right| {
                    left.created_at
                        .cmp(&right.created_at)
                        .then_with(|| left.run_id.cmp(&right.run_id))
                });
            let Some(metadata) = metadata else {
                return false;
            };
            let path = self.repository_path.join(&metadata.worktree_path);
            path.exists()
                && crate::git::inspect_worktree(&path, &self.repository_path)
                    .is_ok_and(|current| current == reviewed.changes)
        });
        let states = blockers
            .iter()
            .map(|blocker| blocker_state(&blocker.status))
            .collect::<Vec<_>>();
        ReviewOperationsSummary {
            run_id: latest_review.map(|run| run.id),
            verdict,
            timestamp: latest_review.map(|run| {
                run.finished_at
                    .clone()
                    .unwrap_or_else(|| run.started_at.clone())
            }),
            applies_to_current_change,
            ready_for_review: task.status == TaskStatus::Review
                && validation.state == ValidationState::Passing,
            actionable_blockers: states.iter().filter(|state| state.is_actionable()).count(),
            unresolved_blockers: states
                .iter()
                .filter(|state| matches!(state, BlockerState::New | BlockerState::Unresolved))
                .count(),
            regressed_blockers: states
                .iter()
                .filter(|state| **state == BlockerState::Regressed)
                .count(),
            resolved_blockers: states
                .iter()
                .filter(|state| **state == BlockerState::Resolved)
                .count(),
        }
    }
}

fn is_active_run(status: &str) -> bool {
    matches!(status, "running" | "waiting_external")
}

fn queue_category_for_status(status: TaskStatus) -> QueueCategory {
    match status {
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

fn next_step(phase: QueueCategory, entry: Option<&QueueEntry>) -> OperationalNextStep {
    match phase {
        QueueCategory::Ready => OperationalNextStep::Dispatch,
        QueueCategory::Active => OperationalNextStep::WaitForExecution,
        QueueCategory::Review => OperationalNextStep::RunSemanticReview,
        QueueCategory::AcceptanceReady => OperationalNextStep::Accept,
        QueueCategory::RevisionRequired => OperationalNextStep::Revise,
        QueueCategory::Blocked if entry.is_some_and(|entry| !entry.waiting_on.is_empty()) => {
            OperationalNextStep::SatisfyDependencies
        }
        QueueCategory::Blocked => OperationalNextStep::ResolveBlocker,
        QueueCategory::Backlog if entry.is_some_and(|entry| !entry.waiting_on.is_empty()) => {
            OperationalNextStep::SatisfyDependencies
        }
        QueueCategory::Backlog => OperationalNextStep::ConfigureEligibleAgent,
        QueueCategory::Done | QueueCategory::Cancelled => OperationalNextStep::None,
    }
}

fn runs_for_task<'a>(facts: &'a ProjectFacts, task_id: &str) -> Vec<&'a AgentRun> {
    facts
        .runs
        .iter()
        .filter(|run| run.task_id.as_deref() == Some(task_id))
        .collect()
}

fn latest_event<'a>(
    facts: &'a ProjectFacts,
    run_id: i64,
    kind: &str,
) -> Option<&'a LifecycleEvent> {
    facts
        .events
        .iter()
        .filter(|event| event.run_id == Some(run_id) && event.kind == kind)
        .max_by_key(|event| event.id)
}

fn blocker_state(status: &str) -> BlockerState {
    match status {
        "new" => BlockerState::New,
        "unresolved" => BlockerState::Unresolved,
        "regression" | "regressed" => BlockerState::Regressed,
        "resolved" => BlockerState::Resolved,
        _ => BlockerState::Unknown,
    }
}

fn blocker_summary(blocker: &ReviewBlockerRecord) -> BlockerSummary {
    let state = blocker_state(&blocker.status);
    BlockerSummary {
        id: blocker.blocker_id.clone(),
        key: blocker.blocker_key.clone(),
        state,
        actionable: state.is_actionable(),
        summary: blocker.finding.clone(),
        requirement: blocker.requirement_ref.clone(),
        evidence: blocker.evidence.clone(),
        severity: blocker.severity.clone(),
        acceptance_condition: blocker.acceptance_condition.clone(),
        originating_review_run_id: blocker.run_id,
        first_seen: blocker.first_seen.clone(),
        last_seen: blocker.last_seen.clone(),
    }
}

fn operational_event(event: &LifecycleEvent) -> OperationalEvent {
    OperationalEvent {
        id: event.id,
        timestamp: event.timestamp.clone(),
        kind: event.kind.clone(),
        run_id: event.run_id,
        agent_id: event.agent_id.clone(),
        payload: event.payload.as_deref().map(|payload| {
            serde_json::from_str(payload)
                .unwrap_or_else(|_| serde_json::Value::String(payload.to_owned()))
        }),
    }
}

fn economy_resolution_summary(item: &ProjectProviderInvocation) -> EconomyResolutionSummary {
    let lineage = item
        .resolution
        .as_ref()
        .map(|resolution| resolution.input_lineage.as_str())
        .unwrap_or(item.invocation.lineage.as_str());
    let lineage: serde_json::Value =
        serde_json::from_str(lineage).unwrap_or(serde_json::Value::Null);
    let quota = lineage.get("quota").and_then(quota_summary);
    let resolution = item.resolution.as_ref();
    EconomyResolutionSummary {
        invocation_id: item.invocation.id,
        run_id: item.invocation.parent_run_id,
        task_id: item.task_id.clone(),
        purpose: item.invocation.purpose.clone(),
        action: lineage["action"].as_str().map(str::to_owned),
        attempt: item.invocation.attempt,
        timestamp: item.invocation.started_at.clone(),
        finished_at: item.invocation.finished_at.clone(),
        outcome: item.invocation.outcome.clone(),
        agent: resolution
            .map(|value| value.selected_agent.clone())
            .or_else(|| item.invocation.selected_agent.clone()),
        model: resolution
            .and_then(|value| value.selected_model.clone())
            .or_else(|| item.invocation.selected_model.clone()),
        effort: resolution
            .and_then(|value| value.effort)
            .or(item.invocation.effort),
        tier: resolution.map_or(item.invocation.tier, |value| value.tier),
        source: resolution.map(|value| value.source.clone()),
        selection_reason: lineage["selection_reason"].as_str().map(str::to_owned),
        selection_explanation: lineage["selection_explanation"].as_str().map(str::to_owned),
        operator_override: ["operator_agent", "operator_model", "operator_effort"]
            .iter()
            .any(|key| !lineage[*key].is_null()),
        escalation_reason: resolution
            .and_then(|value| value.escalation_reason.clone())
            .or_else(|| item.invocation.escalation_reason.clone()),
        quota,
        legacy_missing_resolution: resolution.is_none(),
    }
}

fn quota_summary(value: &serde_json::Value) -> Option<QuotaObservationSummary> {
    value.as_object().map(|_| QuotaObservationSummary {
        remaining_percent: value["remaining_percent"].as_i64(),
        reset_at: value["reset_at"].as_str().map(str::to_owned),
        checked_at: value["checked_at"].as_str().map(str::to_owned),
        source: value["source"].as_str().map(str::to_owned),
        freshness: value["freshness"].as_str().map(str::to_owned),
        reserve_percent: value["reserve_percent"].as_i64(),
        refresh_supported: value["refresh_supported"].as_bool(),
        refresh_error: value["refresh_error"].as_str().map(str::to_owned),
    })
}

fn escalation_summary(
    request: &PersistedEscalationRequest,
    invocations: &[ProjectProviderInvocation],
) -> EscalationSummary {
    let resulting = invocations.iter().find(|item| {
        item.invocation
            .escalation
            .as_ref()
            .and_then(|lineage| lineage.request_id)
            == Some(request.id)
    });
    EscalationSummary {
        request_id: request.id,
        task_id: request.task_id.clone(),
        trigger: request.request.lineage.trigger,
        reason: request.request.reason.clone(),
        previous_provider_invocation_id: request.request.lineage.previous_provider_invocation_id,
        previous_tier: request.request.lineage.previous_tier,
        previous_model: request.request.lineage.previous_model.clone(),
        previous_effort: request.request.lineage.previous_effort,
        previous_attempt: request.request.lineage.previous_attempt,
        requested_minimum_tier: request.request.lineage.requested_minimum_tier,
        policy_attempt: request.request.lineage.policy_attempt,
        state: request.status.clone(),
        created_at: request.created_at.clone(),
        resulting_invocation_id: resulting.map(|item| item.invocation.id),
        resulting_tier: resulting.map(|item| economy_resolution_summary(item).tier),
        resulting_model: resulting.and_then(|item| economy_resolution_summary(item).model),
        resulting_effort: resulting.and_then(|item| economy_resolution_summary(item).effort),
    }
}

#[derive(Default)]
struct TokenUsageAccumulator {
    total: i64,
    input: i64,
    cached: i64,
    output: i64,
    total_known: bool,
    input_known: bool,
    cached_known: bool,
    output_known: bool,
    with_usage: usize,
    without_usage: usize,
}

impl TokenUsageAccumulator {
    fn add(
        &mut self,
        total: Option<i64>,
        input: Option<i64>,
        cached: Option<i64>,
        output: Option<i64>,
    ) {
        if total.is_none() && input.is_none() && cached.is_none() && output.is_none() {
            self.without_usage += 1;
            return;
        }
        self.with_usage += 1;
        if let Some(value) = total {
            self.total += value;
            self.total_known = true;
        }
        if let Some(value) = input {
            self.input += value;
            self.input_known = true;
        }
        if let Some(value) = cached {
            self.cached += value;
            self.cached_known = true;
        }
        if let Some(value) = output {
            self.output += value;
            self.output_known = true;
        }
    }

    fn add_summary(&mut self, summary: &TokenUsageSummary) {
        if let Some(value) = summary.total_tokens {
            self.total += value;
            self.total_known = true;
        }
        if let Some(value) = summary.input_tokens {
            self.input += value;
            self.input_known = true;
        }
        if let Some(value) = summary.cached_input_tokens {
            self.cached += value;
            self.cached_known = true;
        }
        if let Some(value) = summary.output_tokens {
            self.output += value;
            self.output_known = true;
        }
        self.with_usage += summary.observations_with_usage;
        self.without_usage += summary.observations_without_usage;
    }

    fn finish(self) -> TokenUsageSummary {
        let input_tokens = self.input_known.then_some(self.input);
        let cached_input_tokens = self.cached_known.then_some(self.cached);
        TokenUsageSummary {
            total_tokens: self.total_known.then_some(self.total),
            input_tokens,
            cached_input_tokens,
            uncached_input_tokens: input_tokens
                .zip(cached_input_tokens)
                .map(|(input, cached)| input.saturating_sub(cached)),
            output_tokens: self.output_known.then_some(self.output),
            cached_input_ratio: input_tokens
                .filter(|input| *input > 0)
                .zip(cached_input_tokens)
                .map(|(input, cached)| cached as f64 / input as f64),
            observations_with_usage: self.with_usage,
            observations_without_usage: self.without_usage,
        }
    }
}

fn token_usage_for_runs(runs: &[&AgentRun], facts: &ProjectFacts) -> TokenUsageSummary {
    let mut usage = TokenUsageAccumulator::default();
    for run in runs {
        let invocations = facts
            .invocations
            .iter()
            .filter(|item| item.invocation.parent_run_id == run.id)
            .collect::<Vec<_>>();
        let invocation_has_usage = invocations.iter().any(|item| {
            let invocation = &item.invocation;
            invocation.total_tokens.is_some()
                || invocation.input_tokens.is_some()
                || invocation.cached_input_tokens.is_some()
                || invocation.output_tokens.is_some()
        });
        if invocation_has_usage {
            for item in invocations {
                usage.add(
                    item.invocation.total_tokens,
                    item.invocation.input_tokens,
                    item.invocation.cached_input_tokens,
                    item.invocation.output_tokens,
                );
            }
        } else if let Some(result) = facts.results.get(&run.id) {
            usage.add(
                result.total_tokens,
                result.input_tokens,
                result.cached_input_tokens,
                result.output_tokens,
            );
        } else if !invocations.is_empty() {
            for _ in invocations {
                usage.add(None, None, None, None);
            }
        }
    }
    usage.finish()
}
