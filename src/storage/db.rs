use crate::execution::{ExecutionClass, ExecutionTemplate};
use crate::registry::{
    AgentAction, AgentActionProfile, AgentDefinition, QuotaLimits, ReasoningEffort,
};
use crate::task::{Task, TaskPriority, TaskScopeMode, TaskStatus};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use std::{io, path::Path};

pub struct LeadDecisionMetadata<'a> {
    pub snapshot: &'a str,
    pub run_id: Option<i64>,
    pub source_request: &'a str,
    pub summary: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PersistedPlan {
    pub id: i64,
    pub project_id: i64,
    pub version: i64,
    pub parent_plan_id: Option<i64>,
    pub source_lead_decision_id: i64,
    pub source_planner_run_id: i64,
    pub status: PlanStatus,
    pub response: crate::protocol::PlanResponse,
    pub created_at: String,
    pub superseded_by_plan_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanHistoryEntry {
    pub plan_id: i64,
    pub version: i64,
    pub status: PlanStatus,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskExecutionCondition {
    pub task_id: String,
    pub kind: String,
    pub details: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProviderInvocation {
    pub id: i64,
    pub parent_run_id: i64,
    pub workflow_id: Option<i64>,
    pub workflow_stage: Option<String>,
    pub workflow_version: Option<i64>,
    pub purpose: String,
    pub lineage: String,
    pub attempt: usize,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub outcome: String,
    pub effort: Option<ReasoningEffort>,
    pub selected_agent: Option<String>,
    pub selected_model: Option<String>,
    pub escalation_reason: Option<String>,
    pub total_tokens: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanReview {
    pub id: i64,
    pub plan_id: i64,
    pub lead_run_id: i64,
    pub lead_decision_id: i64,
    pub decision: crate::lead::LeadDecisionKind,
    pub details: String,
    pub created_at: String,
    pub superseded_by_review_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlanStatus {
    Proposed,
    UnderReview,
    RevisionRequested,
    Approved,
    Rejected,
    Applied,
    Cancelled,
}
impl PlanStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::UnderReview => "under_review",
            Self::RevisionRequested => "revision_requested",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Applied => "applied",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(value: String) -> rusqlite::Result<Self> {
        match value.as_str() {
            "proposed" => Ok(Self::Proposed),
            "under_review" => Ok(Self::UnderReview),
            "revision_requested" => Ok(Self::RevisionRequested),
            "approved" => Ok(Self::Approved),
            "rejected" => Ok(Self::Rejected),
            "applied" => Ok(Self::Applied),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(rusqlite::Error::InvalidParameterName(format!(
                "invalid plan status: {value}"
            ))),
        }
    }
}

fn priority_string(priority: TaskPriority) -> &'static str {
    match priority {
        TaskPriority::Low => "low",
        TaskPriority::Normal => "normal",
        TaskPriority::High => "high",
        TaskPriority::Critical => "critical",
    }
}

#[cfg(test)]
mod reservation_lifecycle_tests {
    use super::*;
    use crate::task::TaskPriority;
    use tempfile::{TempDir, tempdir};

    fn fixture() -> (TempDir, std::path::PathBuf, Database, i64, String) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("orc.db");
        let db = Database::init(&path).unwrap();
        let project = db.create_project("reservation tests").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        (directory, path, db, project, task)
    }

    fn assert_terminal_releases(status: &str) {
        let (_directory, _path, db, project, task) = fixture();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        assert_eq!(db.list_busy_agents().unwrap(), vec!["codex-main"]);
        db.update_agent_run_status(run, status, Some(status))
            .unwrap();
        assert!(db.list_busy_agents().unwrap().is_empty());
    }

    #[test]
    fn dispatch_marks_agent_busy_while_run_active() {
        let (_directory, _path, db, project, task) = fixture();
        db.create_agent_run(project, &task, "codex-main").unwrap();
        assert_eq!(db.list_busy_agents().unwrap(), vec!["codex-main"]);
    }

    #[test]
    fn successful_dispatch_releases_agent() {
        assert_terminal_releases("completed");
    }

    #[test]
    fn successful_dispatch_and_revision_completion_publish_run_and_task_together() {
        for class in ["implementation", "revision"] {
            let (_directory, _path, db, project, task) = fixture();
            db.update_task_status(&task, TaskStatus::Active).unwrap();
            let run = db
                .create_agent_run_with_execution(
                    project,
                    &task,
                    "codex-main",
                    crate::registry::AUTOMATED,
                    AgentRunExecution {
                        class,
                        model: None,
                        effort: None,
                        source: "test",
                    },
                )
                .unwrap();
            let usage = crate::worker::TokenUsage {
                total_tokens: 30,
                input_tokens: Some(20),
                output_tokens: Some(10),
                cached_input_tokens: Some(6),
            };

            db.complete_agent_run_for_review(&task, run, "validated", Some(usage))
                .unwrap();

            assert_eq!(db.get_agent_run(run).unwrap().unwrap().status, "completed");
            assert_eq!(
                db.get_task(&task).unwrap().unwrap().status,
                TaskStatus::Review
            );
            let result = db.get_worker_result(run).unwrap().unwrap();
            assert_eq!(result.outcome, "success");
            assert_eq!(result.failure_category, None);
            assert_eq!(
                result.metadata.as_deref(),
                Some(r#"{"run_status":"completed"}"#)
            );
            assert_eq!(result.total_tokens, Some(30));
            assert_eq!(result.input_tokens, Some(20));
            assert_eq!(result.output_tokens, Some(10));
            assert_eq!(result.cached_input_tokens, Some(6));
            let events = db.list_lifecycle_events_for_run(run, 10).unwrap();
            let worker_events = events
                .iter()
                .filter(|event| event.kind == "worker_result")
                .collect::<Vec<_>>();
            assert_eq!(worker_events.len(), 1);
            assert_eq!(worker_events[0].task_id.as_deref(), Some(task.as_str()));
            assert_eq!(worker_events[0].agent_id.as_deref(), Some("codex-main"));
            assert_eq!(
                worker_events[0].payload.as_deref(),
                Some(r#"{"outcome":"success"}"#)
            );
            assert!(db.list_busy_agents().unwrap().is_empty());
        }
    }

    #[test]
    fn provider_invocation_is_not_rejected_after_cumulative_tokens_exceed_old_budget() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os("ORC_PROVIDER_TOKEN_BUDGET");
        // Historically, ORC_PROVIDER_TOKEN_BUDGET (default 500_000) capped
        // cumulative provider token usage per run. Setting it here proves the
        // variable no longer has any effect now that enforcement is removed.
        unsafe {
            std::env::set_var("ORC_PROVIDER_TOKEN_BUDGET", "1");
        }
        let (_directory, _path, db, project, task) = fixture();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        let first = db
            .start_provider_invocation(run, "implementation", 1, None)
            .unwrap();
        db.finish_provider_invocation(
            first,
            "completed",
            Some(crate::worker::TokenUsage {
                total_tokens: 900_000,
                input_tokens: Some(800_000),
                output_tokens: Some(100_000),
                cached_input_tokens: Some(50_000),
            }),
        )
        .unwrap();

        // A second invocation on the same parent run, after cumulative usage
        // already far exceeds the old 500_000-token budget, must still be
        // permitted: token accounting is observability only.
        let second = db.start_provider_invocation(run, "completion_repair", 1, None);

        match previous {
            Some(value) => unsafe { std::env::set_var("ORC_PROVIDER_TOKEN_BUDGET", value) },
            None => unsafe { std::env::remove_var("ORC_PROVIDER_TOKEN_BUDGET") },
        }

        assert!(second.is_ok());
        let invocations = db.provider_invocations(run).unwrap();
        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].total_tokens, Some(900_000));
        assert_eq!(invocations[0].cached_input_tokens, Some(50_000));
    }

    #[test]
    fn review_publication_failure_rolls_back_run_completion_and_usage() {
        let (_directory, _path, db, project, task) = fixture();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_review_publication
                 BEFORE UPDATE OF status ON tasks
                 WHEN NEW.id = OLD.id AND NEW.status = 'review'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected review publication failure');
                 END;",
            )
            .unwrap();

        assert!(
            db.complete_agent_run_for_review(&task, run, "validated", None)
                .is_err()
        );

        assert_eq!(db.get_agent_run(run).unwrap().unwrap().status, "running");
        assert_eq!(
            db.get_task(&task).unwrap().unwrap().status,
            TaskStatus::Active
        );
        assert!(db.get_worker_result(run).unwrap().is_none());
        assert_eq!(db.list_busy_agents().unwrap(), vec!["codex-main"]);
        let event_count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_events WHERE run_id = ?1 AND kind = 'worker_result'",
                [run],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 0);
    }

    #[test]
    fn execution_failure_rolls_back_if_task_cannot_be_blocked() {
        let (_directory, _path, db, project, task) = fixture();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_task_block
                 BEFORE UPDATE OF status ON tasks
                 WHEN NEW.id = OLD.id AND NEW.status = 'blocked'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected task block failure');
                 END;",
            )
            .unwrap();

        assert!(db.fail_run(run, "provider failed").is_err());
        assert_eq!(db.get_agent_run(run).unwrap().unwrap().status, "running");
        assert_eq!(
            db.get_task(&task).unwrap().unwrap().status,
            TaskStatus::Active
        );
        assert!(db.get_worker_result(run).unwrap().is_none());
        assert_eq!(db.list_busy_agents().unwrap(), vec!["codex-main"]);
    }

    #[test]
    fn revision_completion_consumes_review_and_contract_with_publication() {
        let (_directory, _path, db, project, task) = fixture();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        let review = db
            .create_agent_run_with_execution(
                project,
                &task,
                "reviewer",
                crate::registry::AUTOMATED,
                AgentRunExecution {
                    class: "review",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        db.update_agent_run_status(review, "completed", Some("review"))
            .unwrap();
        db.persist_revision_contract(&task, review, "contract")
            .unwrap();
        let contract = db.actionable_revision_contract(&task).unwrap().unwrap().2;
        let revision = db
            .create_agent_run_with_execution(
                project,
                &task,
                "codex-main",
                crate::registry::AUTOMATED,
                AgentRunExecution {
                    class: "revision",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();

        assert!(
            db.complete_revision_run_for_review(
                &task,
                revision,
                review,
                Some(contract),
                "validated",
                None,
            )
            .unwrap()
        );

        assert_eq!(db.source_review_run_id(revision).unwrap(), Some(review));
        let consumed: bool = db
            .conn
            .query_row(
                "SELECT review_consumed FROM agent_runs WHERE id = ?1",
                [review],
                |row| row.get(0),
            )
            .unwrap();
        assert!(consumed);
        assert!(db.actionable_revision_contract(&task).unwrap().is_none());
        assert_eq!(
            db.get_agent_run(revision).unwrap().unwrap().status,
            "completed"
        );
        assert_eq!(
            db.get_task(&task).unwrap().unwrap().status,
            TaskStatus::Review
        );
    }

    #[test]
    fn revision_publication_failure_preserves_review_and_contract_actionability() {
        let (_directory, _path, db, project, task) = fixture();
        db.update_task_status(&task, TaskStatus::Active).unwrap();
        let review = db
            .create_agent_run_with_execution(
                project,
                &task,
                "reviewer",
                crate::registry::AUTOMATED,
                AgentRunExecution {
                    class: "review",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        db.update_agent_run_status(review, "completed", Some("review"))
            .unwrap();
        db.persist_revision_contract(&task, review, "contract")
            .unwrap();
        let contract = db.actionable_revision_contract(&task).unwrap().unwrap().2;
        let revision = db
            .create_agent_run_with_execution(
                project,
                &task,
                "codex-main",
                crate::registry::AUTOMATED,
                AgentRunExecution {
                    class: "revision",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        db.conn
            .execute_batch(
                "CREATE TRIGGER fail_revision_publication
                 BEFORE UPDATE OF status ON tasks
                 WHEN NEW.id = OLD.id AND NEW.status = 'review'
                 BEGIN
                     SELECT RAISE(ABORT, 'injected revision publication failure');
                 END;",
            )
            .unwrap();

        assert!(
            db.complete_revision_run_for_review(
                &task,
                revision,
                review,
                Some(contract),
                "validated",
                None,
            )
            .is_err()
        );

        assert_eq!(db.source_review_run_id(revision).unwrap(), None);
        let consumed: bool = db
            .conn
            .query_row(
                "SELECT review_consumed FROM agent_runs WHERE id = ?1",
                [review],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!consumed);
        assert_eq!(
            db.actionable_revision_contract(&task).unwrap().unwrap().2,
            contract
        );
        assert_eq!(
            db.get_agent_run(revision).unwrap().unwrap().status,
            "running"
        );
        assert_eq!(
            db.get_task(&task).unwrap().unwrap().status,
            TaskStatus::Active
        );
        assert!(db.get_worker_result(revision).unwrap().is_none());
        assert_eq!(db.list_busy_agents().unwrap(), vec!["codex-main"]);
    }

    #[test]
    fn failed_dispatch_releases_agent() {
        assert_terminal_releases("failed");
    }
    #[test]
    fn no_changes_releases_agent() {
        assert_terminal_releases("no_changes");
    }
    #[test]
    fn timeout_releases_agent() {
        assert_terminal_releases("timeout");
    }
    #[test]
    fn cancelled_execution_releases_agent() {
        assert_terminal_releases("cancelled");
    }
    #[test]
    fn automated_review_releases_agent_after_completion() {
        assert_terminal_releases("completed");
    }
    #[test]
    fn automated_review_releases_agent_after_failure() {
        assert_terminal_releases("failed");
    }

    #[test]
    fn revision_releases_agent_after_completion_and_failure() {
        assert_terminal_releases("completed");
        assert_terminal_releases("failed");
    }

    #[test]
    fn validation_repair_releases_agent() {
        assert_terminal_releases("failed");
    }

    #[test]
    fn stale_busy_state_is_reconciled_after_reopen() {
        let (_directory, path, db, project, task) = fixture();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        db.conn
            .execute(
                "UPDATE execution_reservations SET owner_pid = 2147483647 WHERE run_id = ?1",
                [run],
            )
            .unwrap();
        drop(db);
        let reopened = Database::open(path).unwrap();
        assert!(reopened.list_busy_agents().unwrap().is_empty());
        assert_eq!(
            reopened.get_agent_run(run).unwrap().unwrap().status,
            "failed"
        );
    }

    #[test]
    fn real_non_terminal_run_remains_busy_after_reconcile() {
        let (_directory, path, db, project, task) = fixture();
        db.create_agent_run(project, &task, "codex-main").unwrap();
        drop(db);
        let reopened = Database::open(path).unwrap();
        assert_eq!(reopened.list_busy_agents().unwrap(), vec!["codex-main"]);
    }

    #[test]
    fn historical_task_assignment_does_not_make_agent_busy() {
        let (_directory, _path, db, project, task) = fixture();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        db.update_agent_run_status(run, "completed", None).unwrap();
        db.update_task_status(&task, TaskStatus::Review).unwrap();
        assert!(db.list_busy_agents().unwrap().is_empty());
        db.update_task_status(&task, TaskStatus::Blocked).unwrap();
        assert!(db.list_busy_agents().unwrap().is_empty());
    }

    #[test]
    fn manually_unavailable_agent_stays_unavailable_after_execution_cleanup() {
        let (_directory, _path, db, project, task) = fixture();
        db.registry.execute(
            "INSERT INTO agents(id, backend, display_name, enabled, priority, capabilities, status) VALUES ('codex-main', 'codex', 'Codex', 1, 1, '[]', 'unavailable')",
            [],
        ).unwrap();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        db.update_agent_run_status(run, "completed", None).unwrap();
        assert_eq!(
            db.get_agent("codex-main").unwrap().unwrap().status,
            "unavailable"
        );
    }

    #[test]
    fn two_sequential_jobs_can_reuse_same_agent() {
        let (_directory, _path, db, project, first) = fixture();
        let one = db.create_agent_run(project, &first, "codex-main").unwrap();
        db.update_agent_run_status(one, "completed", None).unwrap();
        let second = db
            .insert_task(
                project,
                "second",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        assert!(db.create_agent_run(project, &second, "codex-main").is_ok());
    }

    #[test]
    fn terminal_run_cannot_be_overwritten_by_late_timeout() {
        let (_directory, _path, db, project, task) = fixture();
        let run = db.create_agent_run(project, &task, "codex-main").unwrap();
        db.update_agent_run_status(run, "completed", Some("validation completed"))
            .unwrap();
        assert!(
            !db.update_agent_run_status(run, "timeout", Some("late timeout"))
                .unwrap()
        );
        let saved = db.get_agent_run(run).unwrap().unwrap();
        assert_eq!(saved.status, "completed");
        assert_eq!(saved.output.as_deref(), Some("validation completed"));
        assert!(db.list_busy_agents().unwrap().is_empty());
    }

    #[test]
    fn late_terminal_callback_cannot_release_a_newer_run_reservation() {
        let (_directory, _path, db, project, first_task) = fixture();
        let first = db
            .create_agent_run(project, &first_task, "codex-main")
            .unwrap();
        db.update_agent_run_status(first, "completed", Some("done"))
            .unwrap();

        let second_task = db
            .insert_task(
                project,
                "second",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let second = db
            .create_agent_run(project, &second_task, "codex-main")
            .unwrap();

        assert!(
            !db.update_agent_run_status(first, "timeout", Some("late timeout"))
                .unwrap()
        );
        assert_eq!(db.list_busy_agents().unwrap(), vec!["codex-main"]);
        assert_eq!(db.get_agent_run(second).unwrap().unwrap().status, "running");
    }

    #[test]
    fn simultaneous_execution_protection() {
        let (_directory, _path, db, project, first) = fixture();
        db.create_agent_run(project, &first, "codex-main").unwrap();
        let second = db
            .insert_task(
                project,
                "second",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        assert!(matches!(
            db.create_agent_run(project, &second, "codex-main"),
            Err(DbError::AgentHasActiveRun(agent)) if agent == "codex-main"
        ));
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRun {
    pub id: i64,
    pub project_id: i64,
    pub task_id: Option<String>,
    pub agent: String,
    pub execution_mode: String,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub phase: Option<String>,
    pub last_activity: String,
    pub execution_class: String,
    pub resolved_model: Option<String>,
    pub resolved_reasoning_effort: Option<ReasoningEffort>,
    pub resolution_source: String,
    pub resolved_profile: Option<String>,
}

/// Provider authentication evidence and operator-granted permissions are
/// stored separately from provider capabilities in `agents`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentAuthorization {
    pub authenticated: bool,
    pub authentication_method: String,
    pub authentication_detail: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct AgentRunExecution<'a> {
    pub class: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<ReasoningEffort>,
    pub source: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkerResult {
    pub run_id: i64,
    pub outcome: String,
    pub failure_category: Option<String>,
    pub duration_ms: Option<i64>,
    pub metadata: Option<String>,
    pub total_tokens: Option<i64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewBlockerRecord {
    pub task_id: String,
    pub blocker_id: String,
    pub run_id: i64,
    pub requirement_ref: String,
    pub evidence: String,
    pub severity: String,
    pub acceptance_condition: String,
    pub status: String,
    pub finding: String,
    pub first_seen: String,
    pub last_seen: String,
    pub blocker_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LifecycleEvent {
    pub id: i64,
    pub timestamp: String,
    pub kind: String,
    pub task_id: Option<String>,
    pub run_id: Option<i64>,
    pub agent_id: Option<String>,
    pub payload: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ApprovalRequest {
    pub id: i64,
    pub reason: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectAgentReference {
    pub project_id: i64,
    pub agent_id: String,
    pub created_at: String,
}

fn lead_proposal_kind(proposal: &crate::lead::LeadProposalKind) -> &'static str {
    match proposal {
        crate::lead::LeadProposalKind::Plan(_) => "plan",
        crate::lead::LeadProposalKind::Task(_) => "task",
        crate::lead::LeadProposalKind::Revision { .. } => "revision",
        crate::lead::LeadProposalKind::ApprovalRequest { .. } => "approval_request",
    }
}

fn lead_proposal_status(status: crate::lead::LeadProposalStatus) -> &'static str {
    match status {
        crate::lead::LeadProposalStatus::Pending => "pending",
        crate::lead::LeadProposalStatus::Applying => "applying",
        crate::lead::LeadProposalStatus::Applied => "applied",
        crate::lead::LeadProposalStatus::Rejected => "rejected",
        crate::lead::LeadProposalStatus::Superseded => "superseded",
    }
}

fn lead_decision_kind(kind: crate::lead::LeadDecisionKind) -> &'static str {
    match kind {
        crate::lead::LeadDecisionKind::DirectTasks => "DIRECT_TASKS",
        crate::lead::LeadDecisionKind::PlanRequired => "PLAN_REQUIRED",
        crate::lead::LeadDecisionKind::UserDecisionRequired => "USER_DECISION_REQUIRED",
        crate::lead::LeadDecisionKind::Approve => "APPROVE",
        crate::lead::LeadDecisionKind::RevisePlan => "REVISE_PLAN",
    }
}
fn parse_lead_decision_kind(value: &str) -> Result<crate::lead::LeadDecisionKind, rusqlite::Error> {
    match value {
        "DIRECT_TASKS" | "direct_tasks" => Ok(crate::lead::LeadDecisionKind::DirectTasks),
        "PLAN_REQUIRED" | "plan_required" => Ok(crate::lead::LeadDecisionKind::PlanRequired),
        "USER_DECISION_REQUIRED" | "user_decision_required" => {
            Ok(crate::lead::LeadDecisionKind::UserDecisionRequired)
        }
        "APPROVE" | "approve" => Ok(crate::lead::LeadDecisionKind::Approve),
        "REVISE_PLAN" | "revise_plan" => Ok(crate::lead::LeadDecisionKind::RevisePlan),
        _ => Err(rusqlite::Error::InvalidColumnType(
            1,
            "kind".into(),
            rusqlite::types::Type::Text,
        )),
    }
}

fn lead_proposal_from_row(row: &Row<'_>) -> rusqlite::Result<crate::lead::LeadProposal> {
    let proposal = serde_json::from_str(&row.get::<_, String>(1)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let status = match row.get::<_, String>(2)?.as_str() {
        "pending" => crate::lead::LeadProposalStatus::Pending,
        "applying" => crate::lead::LeadProposalStatus::Applying,
        "applied" => crate::lead::LeadProposalStatus::Applied,
        "rejected" => crate::lead::LeadProposalStatus::Rejected,
        value => {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "invalid Lead proposal status: {value}"
            )));
        }
    };
    Ok(crate::lead::LeadProposal {
        id: row.get(0)?,
        proposal,
        status,
        created_at: row.get(3)?,
        applying_at: row.get(4)?,
        resolved_at: row.get(5)?,
    })
}

#[derive(thiserror::Error, Debug)]
pub enum DbError {
    #[error("database filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("invalid next task id in database: {0}")]
    InvalidSequence(String),
    #[error("quota remaining percent must be between 0 and 100, got {0}")]
    InvalidQuota(i64),
    #[error("invalid or already completed agent run: {0}")]
    InvalidRunStatus(i64),
    #[error("task '{0}' cannot depend on itself")]
    SelfDependency(String),
    #[error("task '{0}' not found")]
    TaskNotFound(String),
    #[error("task '{0}' already depends on '{1}'")]
    DuplicateDependency(String, String),
    #[error("dependency cycle detected: adding '{0}' -> '{1}' would create a cycle")]
    DependencyCycle(String, String),
    #[error("dependency '{0}' -> '{1}' not found")]
    DependencyNotFound(String, String),
    #[error("scheduler error: {0}")]
    Scheduler(String),
    #[error("task '{0}' is not active")]
    TaskNotActive(String),
    #[error("task '{0}' has no non-terminal agent run to recover")]
    NoRecoverableRun(String),
    #[error("agent '{0}' has an active run")]
    AgentHasActiveRun(String),
    #[error("agent '{0}' is already archived")]
    AgentAlreadyArchived(String),
    #[error("agent '{0}' not found")]
    AgentNotFound(String),
    #[error("project '{0}' not found")]
    ProjectNotFound(i64),
    #[error("agent '{0}' cannot be purged while it has an active run")]
    AgentPurgeActiveRun(String),
    #[error("task '{0}' cannot be purged while it has an active run")]
    TaskPurgeActiveRun(String),
    #[error("task '{0}' is not terminal")]
    TaskPurgeNotTerminal(String),
    #[error("task '{0}' has dependent tasks: {1}")]
    TaskPurgeHasDependents(String, String),
}

pub struct Database {
    conn: Connection,
    registry: Connection,
    lifecycle_sink: Option<std::sync::Arc<dyn Fn(LifecycleEvent) + Send + Sync>>,
}

/// Last-resort cleanup for automated execution paths. Normal completion uses
/// the explicit terminal status APIs; dropping this guard only terminalizes a
/// run that is still active because an early `?`, panic unwind, or callback
/// error escaped the normal finalization boundary.
pub struct RunFinalizer<'a> {
    db: &'a Database,
    run_id: i64,
}

impl Drop for RunFinalizer<'_> {
    fn drop(&mut self) {
        let _ = self.db.abandon_agent_run(
            self.run_id,
            "execution ended without recording a terminal outcome",
        );
    }
}

impl Database {
    pub fn store_plan(
        &self,
        project_id: i64,
        source_lead_decision_id: i64,
        source_planner_run_id: i64,
        response: &crate::protocol::PlanResponse,
    ) -> Result<i64, DbError> {
        response
            .validate()
            .map_err(|error| DbError::Scheduler(format!("invalid plan: {error}")))?;
        let transaction = self.conn.unchecked_transaction()?;
        let lead_project: Option<i64> = transaction
            .query_row(
                "SELECT project_id FROM lead_decisions WHERE id = ?1 AND kind = 'PLAN_REQUIRED'",
                [source_lead_decision_id],
                |row| row.get(0),
            )
            .optional()?;
        if lead_project != Some(project_id) {
            return Err(DbError::Scheduler(
                "invalid source Lead decision linkage".into(),
            ));
        }
        let run_project: Option<i64> = transaction
            .query_row(
                "SELECT project_id FROM agent_runs WHERE id = ?1 AND execution_class = 'plan'",
                [source_planner_run_id],
                |row| row.get(0),
            )
            .optional()?;
        if run_project != Some(project_id) {
            return Err(DbError::Scheduler(
                "invalid source Planner run linkage".into(),
            ));
        }
        let (parent_plan_id, version): (Option<i64>, i64) = transaction.query_row(
            "SELECT id, version FROM plans WHERE project_id = ?1 ORDER BY version DESC, id DESC LIMIT 1",
            [project_id],
            |row| Ok((Some(row.get(0)?), row.get::<_, i64>(1)? + 1)),
        ).optional()?.unwrap_or((None, 1));
        let canonical = serde_json::to_string(response)?;
        transaction.execute(
            "INSERT INTO plans (project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, PlanStatus::Proposed.as_str(), canonical],
        )?;
        let id = transaction.last_insert_rowid();
        if let Some(parent_plan_id) = parent_plan_id {
            transaction.execute(
                "UPDATE plans SET status = 'cancelled', superseded_by_plan_id = ?1 WHERE id = ?2 AND project_id = ?3 AND status IN ('proposed', 'under_review', 'revision_requested', 'approved')",
                params![id, parent_plan_id, project_id],
            )?;
        }
        for task in &response.tasks {
            for dependency in &task.depends_on {
                transaction.execute(
                    "INSERT INTO plan_dependencies (plan_id, task_local_id, depends_on_local_id) VALUES (?1, ?2, ?3)",
                    params![id, task.local_id, dependency],
                )?;
            }
        }
        transaction.commit()?;
        Ok(id)
    }

    /// Stores a plan and consumes the exact source decision in one transaction.
    /// The conditional update prevents a decision that changed while Planner ran
    /// from being consumed or leaving a plan behind.
    pub fn store_plan_and_consume_decision(
        &self,
        project_id: i64,
        source_lead_decision_id: i64,
        source_planner_run_id: i64,
        response: &crate::protocol::PlanResponse,
    ) -> Result<i64, DbError> {
        response
            .validate()
            .map_err(|error| DbError::Scheduler(format!("invalid plan: {error}")))?;
        let transaction = self.conn.unchecked_transaction()?;
        let valid: Option<i64> = transaction.query_row(
            "SELECT project_id FROM lead_decisions WHERE id = ?1 AND project_id = ?2 AND kind = 'PLAN_REQUIRED' AND status = 'pending'",
            params![source_lead_decision_id, project_id], |row| row.get(0)).optional()?;
        if valid != Some(project_id) {
            return Err(DbError::Scheduler(
                "pending Lead decision changed while Planner was running".into(),
            ));
        }
        let run_project: Option<i64> = transaction
            .query_row(
                "SELECT project_id FROM agent_runs WHERE id = ?1 AND execution_class = 'plan'",
                [source_planner_run_id],
                |row| row.get(0),
            )
            .optional()?;
        if run_project != Some(project_id) {
            return Err(DbError::Scheduler(
                "invalid source Planner run linkage".into(),
            ));
        }
        let (parent_plan_id, version): (Option<i64>, i64) = transaction.query_row(
            "SELECT id, version FROM plans WHERE project_id = ?1 ORDER BY version DESC, id DESC LIMIT 1", [project_id],
            |row| Ok((Some(row.get(0)?), row.get::<_, i64>(1)? + 1))).optional()?.unwrap_or((None, 1));
        let canonical = serde_json::to_string(response)?;
        transaction.execute("INSERT INTO plans (project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, PlanStatus::Proposed.as_str(), canonical])?;
        let id = transaction.last_insert_rowid();
        if let Some(parent_plan_id) = parent_plan_id {
            transaction.execute(
                "UPDATE plans SET status = 'cancelled', superseded_by_plan_id = ?1 WHERE id = ?2 AND project_id = ?3 AND status IN ('proposed', 'under_review', 'revision_requested', 'approved')",
                params![id, parent_plan_id, project_id],
            )?;
        }
        for task in &response.tasks {
            for dependency in &task.depends_on {
                transaction.execute("INSERT INTO plan_dependencies (plan_id, task_local_id, depends_on_local_id) VALUES (?1, ?2, ?3)", params![id, task.local_id, dependency])?;
            }
        }
        let changed = transaction.execute("UPDATE lead_decisions SET status = 'consumed', resolved_at = CURRENT_TIMESTAMP WHERE id = ?1 AND project_id = ?2 AND status = 'pending'", params![source_lead_decision_id, project_id])?;
        if changed != 1 {
            return Err(DbError::Scheduler(
                "pending Lead decision changed while Planner was running".into(),
            ));
        }
        transaction.commit()?;
        Ok(id)
    }

    pub fn get_plan(&self, id: i64) -> Result<Option<PersistedPlan>, DbError> {
        Ok(self.conn.query_row(
            "SELECT id, project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response, created_at, superseded_by_plan_id FROM plans WHERE id = ?1",
            [id],
            |row| Ok(PersistedPlan { id: row.get(0)?, project_id: row.get(1)?, version: row.get(2)?, parent_plan_id: row.get(3)?, source_lead_decision_id: row.get(4)?, source_planner_run_id: row.get(5)?, status: PlanStatus::parse(row.get(6)?)?, response: serde_json::from_str(&row.get::<_, String>(7)?).map_err(|error| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error)))?, created_at: row.get(8)?, superseded_by_plan_id: row.get(9)? }))
        .optional()?)
    }

    /// Checks that a plan is the project's current, actionable Planner output.
    /// This is deliberately performed before any Lead run is created.
    pub fn is_current_valid_planner_plan(
        &self,
        project_id: i64,
        plan: &PersistedPlan,
    ) -> Result<bool, DbError> {
        if plan.project_id != project_id
            || plan.status != PlanStatus::Proposed
            || plan.response.validate().is_err()
        {
            return Ok(false);
        }
        let current: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM plans WHERE project_id = ?1 ORDER BY version DESC, id DESC LIMIT 1",
                [project_id],
                |row| row.get(0),
            )
            .optional()?;
        if current != Some(plan.id) {
            return Ok(false);
        }
        let valid: Option<i64> = self
            .conn
            .query_row(
                "SELECT p.id FROM plans p
             JOIN lead_decisions d ON d.id = p.source_lead_decision_id
             JOIN agent_runs r ON r.id = p.source_planner_run_id
             WHERE p.id = ?1 AND p.project_id = ?2
               AND d.project_id = p.project_id AND d.kind = 'PLAN_REQUIRED'
               AND d.status = 'consumed'
               AND r.project_id = p.project_id AND r.execution_class = 'plan'
               AND r.status = 'completed'",
                params![plan.id, project_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(valid == Some(plan.id))
    }

    pub fn list_plan_history(&self, project_id: i64) -> Result<Vec<PlanHistoryEntry>, DbError> {
        let mut statement = self.conn.prepare("SELECT id, version, status, created_at FROM plans WHERE project_id = ?1 ORDER BY version, id")?;
        Ok(statement
            .query_map([project_id], |row| {
                Ok(PlanHistoryEntry {
                    plan_id: row.get(0)?,
                    version: row.get(1)?,
                    status: PlanStatus::parse(row.get(2)?)?,
                    created_at: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn list_plan_reviews(&self, project_id: i64) -> Result<Vec<PlanReview>, DbError> {
        let mut statement = self.conn.prepare("SELECT r.id, r.plan_id, r.lead_run_id, r.lead_decision_id, r.decision, r.details, r.created_at, r.superseded_by_review_id FROM plan_reviews r JOIN plans p ON p.id = r.plan_id WHERE p.project_id = ?1 ORDER BY r.id")?;
        Ok(statement
            .query_map([project_id], |row| {
                Ok(PlanReview {
                    id: row.get(0)?,
                    plan_id: row.get(1)?,
                    lead_run_id: row.get(2)?,
                    lead_decision_id: row.get(3)?,
                    decision: parse_lead_decision_kind(&row.get::<_, String>(4)?)?,
                    details: row.get(5)?,
                    created_at: row.get(6)?,
                    superseded_by_review_id: row.get(7)?,
                })
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn list_plan_dependencies(&self, plan_id: i64) -> Result<Vec<(String, String)>, DbError> {
        let mut statement = self.conn.prepare(
            "SELECT task_local_id, depends_on_local_id FROM plan_dependencies WHERE plan_id = ?1 ORDER BY task_local_id, depends_on_local_id",
        )?;
        Ok(statement
            .query_map([plan_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn run_finalizer(&self, run_id: i64) -> RunFinalizer<'_> {
        RunFinalizer { db: self, run_id }
    }

    fn abandon_agent_run(&self, run_id: i64, reason: &str) -> Result<bool, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE agent_runs SET status = 'failed', error = ?1, output = COALESCE(output, ?1), finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')",
            params![reason, run_id],
        )?;
        if changed != 0 {
            transaction.execute(
                "DELETE FROM execution_reservations WHERE run_id = ?1",
                [run_id],
            )?;
        }
        transaction.commit()?;
        Ok(changed != 0)
    }
    pub fn project_report(
        &self,
        project_id: i64,
        name: String,
        repository: String,
        engineering_contract: String,
        architecture: crate::protocol::ReportArchitecture,
    ) -> Result<crate::protocol::ProjectReport, DbError> {
        let tasks = self.list_tasks()?;
        let mut counts = std::collections::BTreeMap::new();
        for task in &tasks {
            *counts.entry(task.status.to_string()).or_insert(0) += 1;
        }
        let summaries = tasks
            .iter()
            .map(|task| crate::protocol::TaskSummary {
                id: task.id.clone(),
                title: task.title.clone(),
                status: task.status.to_string(),
            })
            .collect();
        let busy: std::collections::HashSet<_> = self.list_busy_agents()?.into_iter().collect();
        let agents = self
            .list_schedulable_agents()?
            .into_iter()
            .map(|agent| crate::protocol::ReportAgent {
                id: agent.id.clone(),
                display_name: agent.display_name,
                enabled: agent.enabled,
                status: agent.status,
                execution_mode: agent.execution_mode,
                capabilities: agent.capabilities,
                busy: busy.contains(&agent.id),
            })
            .collect();
        let recent_work = self
            .list_agent_runs(project_id, 20)?
            .into_iter()
            .map(|run| crate::protocol::ReportRun {
                task_id: run.task_id,
                agent: run.agent,
                status: run.status,
                output: run.output,
                finished_at: run.finished_at,
            })
            .collect();
        Ok(crate::protocol::ProjectReport {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            project: crate::protocol::ReportProject {
                name,
                repository,
                branch: None,
                commit: None,
            },
            engineering_contract,
            architecture,
            lifecycle: crate::protocol::ReportLifecycle {
                counts,
                tasks: summaries,
            },
            agents,
            queue: crate::queue::compute_queue(self)
                .map_err(|e| DbError::Scheduler(e.to_string()))?,
            recent_work,
            risks: Vec::new(),
            open_questions: Vec::new(),
            role_boundaries: vec![
                "Planner proposes a plan; Orc and humans apply or dispatch it.".into(),
            ],
            planning_constraints: vec![
                "Planning is read-only and must not mutate project state or dispatch work.".into(),
            ],
            approval_requirements: vec![
                "A human must review and approve the plan before ApplyPlan.".into(),
            ],
        })
    }

    pub fn planning_project_state(&self) -> Result<crate::protocol::PlanningProjectState, DbError> {
        let tasks = self.list_tasks()?;
        let queue =
            crate::queue::compute_queue(self).map_err(|e| DbError::Scheduler(e.to_string()))?;
        let mut task_counts = std::collections::BTreeMap::new();
        for task in &tasks {
            *task_counts.entry(task.status.to_string()).or_insert(0) += 1;
        }
        let summaries = |status: &str| {
            tasks
                .iter()
                .filter(|task| task.status.to_string() == status)
                .map(|task| crate::protocol::TaskSummary {
                    id: task.id.clone(),
                    title: task.title.clone(),
                    status: status.into(),
                })
                .collect()
        };
        let queue_summaries = |entries: &[crate::queue::QueueEntry], status: &str| {
            entries
                .iter()
                .map(|entry| crate::protocol::TaskSummary {
                    id: entry.task.id.clone(),
                    title: entry.task.title.clone(),
                    status: status.into(),
                })
                .collect()
        };
        let busy_agents = self.list_busy_agents()?;
        let busy: std::collections::HashSet<_> = busy_agents.iter().cloned().collect();
        let usable_agents = self
            .list_schedulable_agents()?
            .into_iter()
            .filter(|agent| {
                agent.enabled && agent.status == "available" && !busy.contains(&agent.id)
            })
            .map(|agent| agent.id)
            .collect();
        Ok(crate::protocol::PlanningProjectState {
            task_counts,
            ready_tasks: queue_summaries(&queue.ready, "ready"),
            active_tasks: summaries("active"),
            review_tasks: summaries("review"),
            blocked_tasks: queue_summaries(&queue.blocked, "blocked"),
            usable_agents,
            busy_agents,
            quota_reserve_percent: self.quota_reserve()?,
        })
    }

    pub fn project_facts(
        &self,
        project_id: i64,
    ) -> Result<std::collections::BTreeMap<String, String>, DbError> {
        let mut statement = self
            .conn
            .prepare("SELECT key, value FROM project_facts WHERE project_id = ?1 ORDER BY key")?;
        Ok(statement
            .query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn apply_plan(
        &self,
        project_id: i64,
        response: &crate::protocol::PlanResponse,
    ) -> Result<std::collections::BTreeMap<String, String>, DbError> {
        response
            .validate()
            .map_err(|e| DbError::Scheduler(e.to_string()))?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = self.apply_plan_in_transaction(project_id, response);
        match result {
            Ok(mapping) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(mapping)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn apply_plan_in_transaction(
        &self,
        project_id: i64,
        response: &crate::protocol::PlanResponse,
    ) -> Result<std::collections::BTreeMap<String, String>, DbError> {
        let result = (|| {
            let mut mapping = std::collections::BTreeMap::new();
            for task in &response.tasks {
                let id = self.allocate_task_id()?;
                self.insert_task_from_proposal(project_id, &id, task)?;
                mapping.insert(task.local_id.clone(), id);
            }
            for task in &response.tasks {
                for dependency in &task.depends_on {
                    self.add_task_dependency(
                        mapping[&task.local_id].as_str(),
                        mapping[dependency].as_str(),
                    )?;
                }
            }
            self.record_lifecycle_event(
                "plan_applied",
                None,
                None,
                None,
                Some(&format!("{{\"task_count\":{}}}", response.tasks.len())),
            )?;
            Ok::<_, DbError>(mapping)
        })();
        match result {
            Ok(mapping) => Ok(mapping),
            Err(error) => Err(error),
        }
    }

    fn insert_task_from_proposal(
        &self,
        project_id: i64,
        id: &str,
        task: &crate::protocol::TaskProposal,
    ) -> Result<(), DbError> {
        let effort =
            task.execution_hints.effort.as_deref().ok_or_else(|| {
                DbError::Scheduler("task proposal has no execution effort".into())
            })?;
        let capabilities = crate::registry::normalize_capability_names(&task.capabilities);
        self.conn.execute(
            "INSERT INTO tasks (id, project_id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, execution_class, execution_model, reasoning_effort, effort_reason, risk_factors, acceptance_criteria, required_tests, validation, unchanged) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                id,
                project_id,
                task.title,
                task.objective,
                task.role,
                priority_string(task.priority),
                serde_json::to_string(&capabilities)?,
                task.scope_mode.map(|value| value.to_string()),
                serde_json::to_string(&task.context_files)?,
                serde_json::to_string(&task.expected_changes)?,
                task.execution_hints.class,
                task.execution_hints.model,
                effort,
                task.execution_hints.effort_reason,
                serde_json::to_string(&task.risk_factors)?,
                serde_json::to_string(&task.acceptance_criteria)?,
                serde_json::to_string(&task.required_tests)?,
                serde_json::to_string(&task.validation)?,
                serde_json::to_string(&task.unchanged)?,
            ],
        )?;
        let mut persisted_proposal = task.clone();
        persisted_proposal.capabilities = capabilities;
        self.conn.execute(
            "INSERT INTO task_proposal_metadata (task_id, proposal) VALUES (?1, ?2)",
            params![id, serde_json::to_string(&persisted_proposal)?],
        )?;
        self.record_lifecycle_event("task_created", Some(id), None, None, None)?;
        Ok(())
    }
    pub fn apply_approved_plan(
        &self,
        project_id: i64,
    ) -> Result<std::collections::BTreeMap<String, String>, DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let plan = self.conn.query_row("SELECT id, project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response, created_at, superseded_by_plan_id FROM plans WHERE project_id=?1 ORDER BY version DESC, id DESC LIMIT 1", [project_id], |r| Ok(PersistedPlan { id:r.get(0)?, project_id:r.get(1)?, version:r.get(2)?, parent_plan_id:r.get(3)?, source_lead_decision_id:r.get(4)?, source_planner_run_id:r.get(5)?, status:PlanStatus::parse(r.get(6)?)?, response:serde_json::from_str(&r.get::<_,String>(7)?).map_err(|e| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e)))?, created_at:r.get(8)?, superseded_by_plan_id:r.get(9)? })).optional()?.ok_or_else(|| DbError::Scheduler("no Planner plan found".into()))?;
            if plan.status != PlanStatus::Approved {
                return Err(DbError::Scheduler(
                    "current Planner plan is not approved".into(),
                ));
            }
            plan.response
                .validate()
                .map_err(|e| DbError::Scheduler(e.to_string()))?;
            let mapping = self.apply_plan_in_transaction(project_id, &plan.response)?;
            if self.conn.execute(
                "UPDATE plans SET status='applied' WHERE id=?1 AND status='approved'",
                [plan.id],
            )? != 1
            {
                return Err(DbError::Scheduler(
                    "Planner plan was already applied".into(),
                ));
            }
            Ok(mapping)
        })();
        match result {
            Ok(mapping) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(mapping)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn init(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let registry_path = Self::companion_registry_path(path.as_ref());
        Self::init_with_registry(path, registry_path)
    }

    pub fn init_with_registry(
        path: impl AsRef<Path>,
        registry_path: impl AsRef<Path>,
    ) -> Result<Self, DbError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path.as_ref())?;
        Self::configure(&conn)?;
        conn.execute_batch(
            r#"
            BEGIN;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS projects (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS project_facts (
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                PRIMARY KEY (project_id, key)
            );
            CREATE TABLE IF NOT EXISTS discovery_snapshots (
                project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
                fingerprint TEXT NOT NULL,
                snapshot TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                model_version INTEGER NOT NULL DEFAULT 1,
                scope TEXT NOT NULL DEFAULT 'global',
                backend TEXT NOT NULL,
                display_name TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0,
                capabilities TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'available',
                unavailable_reason TEXT,
                profile_path TEXT,
                model TEXT,
                reasoning_effort TEXT,
                config_metadata TEXT,
                execution_mode TEXT NOT NULL DEFAULT 'automated',
                quota_remaining_percent INTEGER,
                quota_reset_at TEXT,
                quota_checked_at TEXT,
                quota_source TEXT
                , quota_limits TEXT
            );
            CREATE TABLE IF NOT EXISTS project_agent_references (
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                agent_id TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                PRIMARY KEY (project_id, agent_id)
            );
            CREATE TABLE IF NOT EXISTS agent_authorizations (
                agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
                permissions TEXT NOT NULL DEFAULT '[]',
                authenticated INTEGER NOT NULL DEFAULT 0,
                authentication_method TEXT NOT NULL DEFAULT 'unknown',
                authentication_detail TEXT,
                verified_at TEXT
            );
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                title TEXT NOT NULL,
                objective TEXT NOT NULL,
                role TEXT NOT NULL,
                priority TEXT NOT NULL,
                status TEXT NOT NULL,
                required_capabilities TEXT,
                scope_mode TEXT,
                context_files TEXT,
                expected_changes TEXT,
                execution_class TEXT,
                execution_model TEXT,
                reasoning_effort TEXT,
                effort_reason TEXT,
                risk_factors TEXT,
                acceptance_criteria TEXT,
                required_tests TEXT,
                validation TEXT,
                unchanged TEXT,
                cancellation_reason TEXT,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                updated_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS task_dependencies (
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                depends_on TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                PRIMARY KEY (task_id, depends_on)
            );
            CREATE TABLE IF NOT EXISTS task_proposal_metadata (
                task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                proposal TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS task_execution_conditions (
                task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                details TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS decisions (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                task_id TEXT REFERENCES tasks(id),
                summary TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS approval_requests (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                reason TEXT NOT NULL,
                resolved INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS agent_runs (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id),
                task_id TEXT REFERENCES tasks(id),
                agent TEXT NOT NULL,
                execution_mode TEXT NOT NULL DEFAULT 'automated',
                status TEXT NOT NULL,
                output TEXT,
                error TEXT,
                started_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                finished_at TEXT
                , phase TEXT
                , last_activity TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE TABLE IF NOT EXISTS plans (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                version INTEGER NOT NULL,
                parent_plan_id INTEGER REFERENCES plans(id),
                source_lead_decision_id INTEGER NOT NULL,
                source_planner_run_id INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'proposed',
                response TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                superseded_by_plan_id INTEGER REFERENCES plans(id),
                UNIQUE(project_id, version)
            );
            CREATE TABLE IF NOT EXISTS plan_dependencies (
                plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
                task_local_id TEXT NOT NULL,
                depends_on_local_id TEXT NOT NULL,
                PRIMARY KEY(plan_id, task_local_id, depends_on_local_id)
            );
            CREATE TABLE IF NOT EXISTS worker_results (
                run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE,
                outcome TEXT NOT NULL,
                failure_category TEXT,
                duration_ms INTEGER,
                metadata TEXT,
                total_tokens INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                cached_input_tokens INTEGER
            );
            CREATE TABLE IF NOT EXISTS worker_protocol_results (
                run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE,
                prepare TEXT NOT NULL,
                execution TEXT
            );
            CREATE TABLE IF NOT EXISTS lifecycle_events (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                kind TEXT NOT NULL,
                task_id TEXT REFERENCES tasks(id),
                run_id INTEGER REFERENCES agent_runs(id),
                agent_id TEXT,
                payload TEXT
            );
            CREATE TABLE IF NOT EXISTS worktree_metadata (
                agent_run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE,
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                branch_name TEXT NOT NULL,
                worktree_path TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            INSERT OR IGNORE INTO meta (key, value) VALUES ('next_task_id', '1');
            COMMIT;
            "#,
        )?;
        Self::ensure_agent_columns(&conn)?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS discovery_snapshots (project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE, fingerprint TEXT NOT NULL, snapshot TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP))")?;
        Self::ensure_project_agent_references_table(&conn)?;
        Self::ensure_execution_templates_table(&conn)?;
        Self::ensure_agent_actions_table(&conn)?;
        Self::ensure_agent_authorizations_table(&conn)?;
        Self::ensure_agent_run_columns(&conn)?;
        Self::ensure_execution_reservations_table(&conn)?;
        Self::ensure_worker_results_table(&conn)?;
        Self::ensure_worker_protocol_results_table(&conn)?;
        Self::ensure_provider_invocations_table(&conn)?;
        Self::ensure_workflow_tables(&conn)?;
        Self::ensure_lifecycle_events_table(&conn)?;
        Self::ensure_worktree_metadata_table(&conn)?;
        Self::ensure_change_evidence_table(&conn)?;
        Self::ensure_review_blockers_table(&conn)?;
        Self::ensure_revision_contracts_table(&conn)?;
        Self::ensure_lead_tables(&conn)?;
        Self::ensure_lead_provider_config_table(&conn)?;
        Self::ensure_plan_tables(&conn)?;
        Self::ensure_task_columns(&conn)?;
        Self::ensure_task_execution_conditions_table(&conn)?;
        Self::ensure_approval_request_columns(&conn)?;
        let registry_path = Self::absolute_registry_path(registry_path.as_ref())?;
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('agent_registry_path', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [registry_path.to_string_lossy().as_ref()],
        )?;
        let registry = Self::open_registry(&registry_path, true)?;
        Self::migrate_legacy_registry(path.as_ref(), &registry)?;
        let db = Self {
            conn,
            registry,
            lifecycle_sink: None,
        };
        db.reconcile_execution_reservations()?;
        Ok(db)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        let registry_path = Self::persisted_registry_path(path.as_ref())?
            .unwrap_or_else(|| Self::companion_registry_path(path.as_ref()));
        Self::open_with_registry(path, registry_path)
    }

    pub fn open_with_registry(
        path: impl AsRef<Path>,
        registry_path: impl AsRef<Path>,
    ) -> Result<Self, DbError> {
        if !path.as_ref().exists() {
            return Err(DbError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("database does not exist: {}", path.as_ref().display()),
            )));
        }
        let conn = Connection::open_with_flags(path.as_ref(), OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        Self::configure(&conn)?;
        Self::ensure_registry_schema(&conn)?;
        Self::ensure_lead_tables(&conn)?;
        Self::ensure_lead_provider_config_table(&conn)?;
        Self::ensure_plan_tables(&conn)?;
        Self::ensure_workflow_tables(&conn)?;
        let registry_path = Self::absolute_registry_path(registry_path.as_ref())?;
        conn.execute(
            "INSERT INTO meta(key, value) VALUES ('agent_registry_path', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [registry_path.to_string_lossy().as_ref()],
        )?;
        let registry = Self::open_registry(&registry_path, true)?;
        Self::migrate_legacy_registry(path.as_ref(), &registry)?;
        let db = Self {
            conn,
            registry,
            lifecycle_sink: None,
        };
        db.reconcile_execution_reservations()?;
        Ok(db)
    }

    fn configure(conn: &Connection) -> Result<(), DbError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Ok(())
    }

    fn companion_registry_path(project_db: &Path) -> std::path::PathBuf {
        let file = project_db
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("orc");
        project_db.with_file_name(format!("{file}.agents.db"))
    }

    fn absolute_registry_path(path: &Path) -> Result<std::path::PathBuf, DbError> {
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            Ok(std::env::current_dir()?.join(path))
        }
    }

    fn persisted_registry_path(project_db: &Path) -> Result<Option<std::path::PathBuf>, DbError> {
        if !project_db.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(project_db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let value = conn
            .query_row(
                "SELECT value FROM meta WHERE key='agent_registry_path'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional();
        match value {
            Ok(value) => Ok(value.map(std::path::PathBuf::from)),
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.contains("no such table") =>
            {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn default_global_registry_path() -> std::path::PathBuf {
        if let Some(path) = std::env::var_os("ORC_GLOBAL_REGISTRY_PATH") {
            return path.into();
        }
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return std::path::PathBuf::from(path).join("orc/agents.db");
        }
        std::env::var_os("HOME").map_or_else(
            || std::path::PathBuf::from(".orc-global/agents.db"),
            |home| std::path::PathBuf::from(home).join(".local/share/orc/agents.db"),
        )
    }

    pub fn init_global(path: impl AsRef<Path>) -> Result<Self, DbError> {
        Self::init_with_registry(path, Self::default_global_registry_path())
    }

    pub fn open_global(path: impl AsRef<Path>) -> Result<Self, DbError> {
        Self::open_with_registry(path, Self::default_global_registry_path())
    }

    fn open_registry(path: &Path, create: bool) -> Result<Connection, DbError> {
        if create && let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let registry = if create {
            Connection::open(path)?
        } else {
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?
        };
        Self::configure(&registry)?;
        registry.execute_batch(
            "CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY, model_version INTEGER NOT NULL DEFAULT 1,
                scope TEXT NOT NULL DEFAULT 'global', backend TEXT NOT NULL,
                display_name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
                priority INTEGER NOT NULL DEFAULT 0, capabilities TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'available', unavailable_reason TEXT,
                profile_path TEXT, model TEXT, reasoning_effort TEXT, config_metadata TEXT,
                execution_mode TEXT NOT NULL DEFAULT 'automated', quota_remaining_percent INTEGER,
                quota_reset_at TEXT, quota_checked_at TEXT, quota_source TEXT, quota_limits TEXT
            );
            CREATE TABLE IF NOT EXISTS agent_action_profiles (
                agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                action TEXT NOT NULL, model TEXT, reasoning_effort TEXT,
                PRIMARY KEY(agent_id, action)
            );
            CREATE TABLE IF NOT EXISTS agent_authorizations (
                agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
                permissions TEXT NOT NULL DEFAULT '[]', authenticated INTEGER NOT NULL DEFAULT 0,
                authentication_method TEXT NOT NULL DEFAULT 'unknown', authentication_detail TEXT,
                verified_at TEXT
            );",
        )?;
        Self::ensure_agent_columns(&registry)?;
        Ok(registry)
    }

    fn migrate_legacy_registry(project_db: &Path, registry: &Connection) -> Result<(), DbError> {
        registry.execute(
            "ATTACH DATABASE ?1 AS legacy_project",
            [project_db.to_string_lossy().as_ref()],
        )?;
        let result = registry.execute_batch(
            "BEGIN IMMEDIATE;
             INSERT OR IGNORE INTO agents
             SELECT id, model_version, scope, backend, display_name, enabled, priority,
                    capabilities, status, unavailable_reason, profile_path, model,
                    reasoning_effort, config_metadata, execution_mode, quota_remaining_percent,
                    quota_reset_at, quota_checked_at, quota_source, quota_limits
             FROM legacy_project.agents
             WHERE NOT EXISTS (
                 SELECT 1 FROM legacy_project.meta WHERE key='agent_registry_migrated'
             );
             INSERT OR IGNORE INTO agent_action_profiles
             SELECT p.agent_id, p.action, p.model, p.reasoning_effort
             FROM legacy_project.agent_action_profiles p JOIN agents a ON a.id=p.agent_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM legacy_project.meta WHERE key='agent_registry_migrated'
             );
             INSERT OR IGNORE INTO agent_authorizations
             SELECT x.agent_id, x.permissions, x.authenticated, x.authentication_method,
                    x.authentication_detail, x.verified_at
             FROM legacy_project.agent_authorizations x JOIN agents a ON a.id=x.agent_id
             WHERE NOT EXISTS (
                 SELECT 1 FROM legacy_project.meta WHERE key='agent_registry_migrated'
             );
             INSERT INTO legacy_project.meta(key, value) VALUES ('agent_registry_migrated', '1')
             ON CONFLICT(key) DO UPDATE SET value=excluded.value;
             COMMIT;",
        );
        if result.is_err() {
            let _ = registry.execute_batch("ROLLBACK");
        }
        let detach = registry.execute_batch("DETACH DATABASE legacy_project");
        result?;
        detach?;
        Ok(())
    }

    fn ensure_registry_schema(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS project_facts (project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (project_id, key)); CREATE TABLE IF NOT EXISTS discovery_snapshots (project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE, fingerprint TEXT NOT NULL, snapshot TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)); CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, model_version INTEGER NOT NULL DEFAULT 1, scope TEXT NOT NULL DEFAULT 'global', backend TEXT NOT NULL, display_name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, priority INTEGER NOT NULL DEFAULT 0, capabilities TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'available', unavailable_reason TEXT, profile_path TEXT, config_metadata TEXT);",
        )?;
        Self::ensure_agent_columns(conn)?;
        Self::ensure_project_agent_references_table(conn)?;
        Self::ensure_execution_templates_table(conn)?;
        Self::ensure_agent_actions_table(conn)?;
        Self::ensure_agent_authorizations_table(conn)?;
        Self::ensure_agent_run_columns(conn)?;
        Self::ensure_execution_reservations_table(conn)?;
        Self::ensure_worker_results_table(conn)?;
        Self::ensure_worker_protocol_results_table(conn)?;
        Self::ensure_provider_invocations_table(conn)?;
        Self::ensure_lifecycle_events_table(conn)?;
        Self::ensure_worktree_metadata_table(conn)?;
        Self::ensure_change_evidence_table(conn)?;
        Self::ensure_review_blockers_table(conn)?;
        Self::ensure_revision_contracts_table(conn)?;
        Self::ensure_task_columns(conn)?;
        Self::ensure_task_execution_conditions_table(conn)?;
        Self::ensure_approval_request_columns(conn)?;
        Ok(())
    }

    fn ensure_execution_reservations_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS execution_reservations (
                agent_id TEXT PRIMARY KEY,
                run_id INTEGER NOT NULL UNIQUE REFERENCES agent_runs(id) ON DELETE CASCADE,
                owner_pid INTEGER,
                acquired_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            )",
        )?;
        Ok(())
    }

    fn ensure_project_agent_references_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS project_agent_references (
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                agent_id TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                PRIMARY KEY (project_id, agent_id)
            )",
        )?;
        let legacy_agent_fk: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_foreign_key_list('project_agent_references') WHERE \"table\"='agents'",
            [],
            |row| row.get(0),
        )?;
        if legacy_agent_fk != 0 {
            conn.pragma_update(None, "foreign_keys", "OFF")?;
            let migration = conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE project_agent_references RENAME TO project_agent_references_legacy;
                 CREATE TABLE project_agent_references (
                    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                    agent_id TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                    PRIMARY KEY (project_id, agent_id)
                 );
                 INSERT INTO project_agent_references SELECT project_id, agent_id, created_at FROM project_agent_references_legacy;
                 DROP TABLE project_agent_references_legacy;
                 COMMIT;",
            );
            conn.pragma_update(None, "foreign_keys", "ON")?;
            migration?;
        }
        Ok(())
    }

    fn process_is_alive(pid: i64) -> bool {
        if pid == i64::from(std::process::id()) {
            return true;
        }
        #[cfg(target_os = "linux")]
        {
            std::path::Path::new("/proc").join(pid.to_string()).exists()
        }
        #[cfg(not(target_os = "linux"))]
        {
            // On platforms without a portable process probe, retain the reservation.
            true
        }
    }

    /// Repairs reservations left by older Orc versions or an execution process
    /// that exited without reaching the shared run-finalization boundary.
    pub fn reconcile_execution_reservations(&self) -> Result<usize, DbError> {
        let mut repaired = 0;
        let reservations = self
            .conn
            .prepare("SELECT agent_id, run_id, owner_pid FROM execution_reservations")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (agent, run, owner_pid) in reservations {
            let run_state: Option<(String, String)> = self
                .conn
                .query_row(
                    "SELECT status, execution_mode FROM agent_runs WHERE id = ?1 AND agent = ?2",
                    params![run, agent],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let active = run_state.as_ref().is_some_and(|(status, _)| {
                matches!(status.as_str(), "running" | "waiting_external")
            });
            let orphaned = match (owner_pid, run_state.as_ref()) {
                (Some(pid), _) => !Self::process_is_alive(pid),
                (None, Some((status, mode))) => {
                    mode != crate::registry::MANUAL || status != "waiting_external"
                }
                (None, None) => true,
            };
            if !active || orphaned {
                if orphaned {
                    self.conn.execute(
                        "UPDATE agent_runs SET status = 'failed', error = 'execution interrupted before completion', output = COALESCE(output, 'execution interrupted before completion'), finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP WHERE id = ?1 AND status IN ('running', 'waiting_external')",
                        [run],
                    )?;
                }
                repaired += self.conn.execute(
                    "DELETE FROM execution_reservations WHERE agent_id = ?1 AND run_id = ?2",
                    params![agent, run],
                )?;
            }
        }

        // Manual waiting-external work intentionally survives Orc processes.
        repaired += self.conn.execute(
            "INSERT OR IGNORE INTO execution_reservations(agent_id, run_id, owner_pid)
             SELECT agent, id, NULL FROM agent_runs
             WHERE execution_mode = 'manual' AND status = 'waiting_external'
               AND NOT EXISTS (SELECT 1 FROM execution_reservations r WHERE r.run_id = agent_runs.id)",
            [],
        )?;
        // A pre-reservation automated row cannot have a live owner. Terminalize it
        // once during migration instead of allowing it to reserve an agent forever.
        repaired += self.conn.execute(
            "UPDATE agent_runs SET status = 'failed', error = 'execution interrupted before reservation tracking', output = COALESCE(output, 'execution interrupted before reservation tracking'), finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP
             WHERE execution_mode = 'automated' AND status IN ('running', 'waiting_external')
               AND NOT EXISTS (SELECT 1 FROM execution_reservations r WHERE r.run_id = agent_runs.id)",
            [],
        )?;
        Ok(repaired)
    }

    fn ensure_lifecycle_events_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS lifecycle_events (id INTEGER PRIMARY KEY, timestamp TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), kind TEXT NOT NULL, task_id TEXT REFERENCES tasks(id), run_id INTEGER REFERENCES agent_runs(id), agent_id TEXT, payload TEXT)")?;
        Ok(())
    }

    fn ensure_execution_templates_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS execution_templates (class TEXT PRIMARY KEY, model TEXT, reasoning_effort TEXT)")?;
        Ok(())
    }

    fn ensure_agent_actions_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_action_profiles (agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE, action TEXT NOT NULL, model TEXT, reasoning_effort TEXT, PRIMARY KEY(agent_id, action))")?;
        Ok(())
    }

    fn ensure_agent_authorizations_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_authorizations (
                agent_id TEXT PRIMARY KEY REFERENCES agents(id) ON DELETE CASCADE,
                permissions TEXT NOT NULL DEFAULT '[]',
                authenticated INTEGER NOT NULL DEFAULT 0,
                authentication_method TEXT NOT NULL DEFAULT 'unknown',
                authentication_detail TEXT,
                verified_at TEXT
            )",
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO agent_authorizations (agent_id) SELECT id FROM agents",
            [],
        )?;
        Ok(())
    }

    pub fn agent_action_profiles(&self, id: &str) -> Result<Vec<AgentActionProfile>, DbError> {
        let mut s = self.registry.prepare("SELECT action, model, reasoning_effort FROM agent_action_profiles WHERE agent_id = ?1 ORDER BY action")?;
        Ok(s.query_map([id], |r| {
            Ok(AgentActionProfile {
                action: AgentAction::parse(&r.get::<_, String>(0)?)
                    .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?,
                model: r.get(1)?,
                reasoning_effort: r
                    .get::<_, Option<String>>(2)?
                    .map(|v| ReasoningEffort::parse(&v))
                    .transpose()
                    .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?,
            })
        })?
        .collect::<Result<_, _>>()?)
    }

    pub fn set_agent_action_profile(
        &self,
        id: &str,
        action: AgentAction,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<bool, DbError> {
        Ok(self.registry.execute("INSERT INTO agent_action_profiles(agent_id, action, model, reasoning_effort) VALUES(?1, ?2, ?3, ?4) ON CONFLICT(agent_id, action) DO UPDATE SET model=excluded.model, reasoning_effort=excluded.reasoning_effort", params![id, action.as_str(), model, effort.map(|v| v.as_str())])? != 0)
    }

    pub fn clear_agent_action_profile(
        &self,
        id: &str,
        action: AgentAction,
    ) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "DELETE FROM agent_action_profiles WHERE agent_id=?1 AND action=?2",
            params![id, action.as_str()],
        )? != 0)
    }

    fn ensure_lead_provider_config_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS lead_provider_config (id INTEGER PRIMARY KEY CHECK (id = 1), agent_id TEXT NOT NULL, model TEXT, reasoning_effort TEXT)")?;
        Ok(())
    }

    pub fn lead_provider_config(&self) -> Result<Option<crate::lead::LeadProviderConfig>, DbError> {
        self.conn
            .query_row(
                "SELECT agent_id, model, reasoning_effort FROM lead_provider_config WHERE id = 1",
                [],
                |row| {
                    let effort = row
                        .get::<_, Option<String>>(2)?
                        .map(|value| {
                            ReasoningEffort::parse(&value).map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    2,
                                    rusqlite::types::Type::Text,
                                    Box::new(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        error.to_string(),
                                    )),
                                )
                            })
                        })
                        .transpose()?;
                    Ok(crate::lead::LeadProviderConfig {
                        agent_id: row.get(0)?,
                        model: row.get(1)?,
                        reasoning_effort: effort,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
    }

    pub fn set_lead_provider_config(
        &self,
        config: &crate::lead::LeadProviderConfig,
    ) -> Result<(), DbError> {
        self.conn.execute("INSERT INTO lead_provider_config (id, agent_id, model, reasoning_effort) VALUES (1, ?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET agent_id = excluded.agent_id, model = excluded.model, reasoning_effort = excluded.reasoning_effort", params![config.agent_id, config.model, config.reasoning_effort.map(|value| value.as_str())])?;
        Ok(())
    }

    pub fn clear_lead_provider_config(&self) -> Result<(), DbError> {
        self.conn
            .execute("DELETE FROM lead_provider_config WHERE id = 1", [])?;
        Ok(())
    }

    pub fn execution_template(&self, class: ExecutionClass) -> Result<ExecutionTemplate, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT model, reasoning_effort FROM execution_templates WHERE class = ?1",
                [class.as_str()],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        Ok(row
            .map(|(model, effort)| ExecutionTemplate {
                model,
                reasoning_effort: effort.and_then(|v| ReasoningEffort::parse(&v).ok()),
            })
            .unwrap_or_default())
    }

    pub fn execution_templates(&self) -> Result<Vec<(ExecutionClass, ExecutionTemplate)>, DbError> {
        ExecutionClass::all()
            .into_iter()
            .map(|class| {
                self.execution_template(class)
                    .map(|template| (class, template))
            })
            .collect()
    }

    pub fn set_execution_template(
        &self,
        class: ExecutionClass,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<(), DbError> {
        self.conn.execute("INSERT INTO execution_templates (class, model, reasoning_effort) VALUES (?1, ?2, ?3) ON CONFLICT(class) DO UPDATE SET model = excluded.model, reasoning_effort = excluded.reasoning_effort", params![class.as_str(), model, effort.map(|v| v.as_str())])?;
        Ok(())
    }

    pub fn clear_execution_template(&self, class: ExecutionClass) -> Result<(), DbError> {
        self.conn.execute(
            "DELETE FROM execution_templates WHERE class = ?1",
            [class.as_str()],
        )?;
        Ok(())
    }

    pub fn record_lifecycle_event(
        &self,
        kind: &str,
        task_id: Option<&str>,
        run_id: Option<i64>,
        agent_id: Option<&str>,
        payload: Option<&str>,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO lifecycle_events (kind, task_id, run_id, agent_id, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![kind, task_id, run_id, agent_id, payload],
        )?;
        let id = self.conn.last_insert_rowid();
        if let Some(sink) = &self.lifecycle_sink {
            sink(LifecycleEvent {
                id,
                timestamp: self.conn.query_row(
                    "SELECT timestamp FROM lifecycle_events WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )?,
                kind: kind.to_owned(),
                task_id: task_id.map(str::to_owned),
                run_id,
                agent_id: agent_id.map(str::to_owned),
                payload: payload.map(str::to_owned),
            });
        }
        Ok(id)
    }

    pub fn set_lifecycle_sink(
        &mut self,
        sink: Option<std::sync::Arc<dyn Fn(LifecycleEvent) + Send + Sync>>,
    ) {
        self.lifecycle_sink = sink;
    }

    pub fn list_lifecycle_events(&self, limit: usize) -> Result<Vec<LifecycleEvent>, DbError> {
        let mut statement = self.conn.prepare("SELECT id, timestamp, kind, task_id, run_id, agent_id, payload FROM lifecycle_events ORDER BY id DESC LIMIT ?1")?;
        Ok(statement
            .query_map(params![limit as i64], |row| {
                Ok(LifecycleEvent {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    kind: row.get(2)?,
                    task_id: row.get(3)?,
                    run_id: row.get(4)?,
                    agent_id: row.get(5)?,
                    payload: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_lifecycle_events_for_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<LifecycleEvent>, DbError> {
        self.list_lifecycle_events_scoped("task_id = ?1", params![task_id], limit)
    }

    pub fn list_lifecycle_events_for_run(
        &self,
        run_id: i64,
        limit: usize,
    ) -> Result<Vec<LifecycleEvent>, DbError> {
        self.list_lifecycle_events_scoped("run_id = ?1", params![run_id], limit)
    }

    pub fn latest_validation_result_for_run(&self, run_id: i64) -> Result<Option<String>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT payload FROM lifecycle_events WHERE run_id = ?1 AND kind = 'validation_result' ORDER BY id DESC LIMIT 1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()?)
    }


    fn list_lifecycle_events_scoped(
        &self,
        predicate: &str,
        values: impl rusqlite::Params,
        limit: usize,
    ) -> Result<Vec<LifecycleEvent>, DbError> {
        let sql = format!(
            "SELECT id, timestamp, kind, task_id, run_id, agent_id, payload FROM lifecycle_events WHERE {predicate} ORDER BY id DESC LIMIT {limit}"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let mut rows = statement.query(values)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            events.push(LifecycleEvent {
                id: row.get(0)?,
                timestamp: row.get(1)?,
                kind: row.get(2)?,
                task_id: row.get(3)?,
                run_id: row.get(4)?,
                agent_id: row.get(5)?,
                payload: row.get(6)?,
            });
        }
        Ok(events)
    }

    fn ensure_approval_request_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(approval_requests)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "resolved") {
            conn.execute_batch(
                "ALTER TABLE approval_requests ADD COLUMN resolved INTEGER NOT NULL DEFAULT 0",
            )?;
        }
        Ok(())
    }

    fn ensure_task_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(tasks)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns
            .iter()
            .any(|column| column == "required_capabilities")
        {
            conn.execute_batch("ALTER TABLE tasks ADD COLUMN required_capabilities TEXT")?;
        }
        for (name, definition) in [
            ("scope_mode", "TEXT"),
            ("context_files", "TEXT"),
            ("expected_changes", "TEXT"),
            ("reasoning_effort", "TEXT"),
            ("execution_class", "TEXT"),
            ("execution_model", "TEXT"),
            ("effort_reason", "TEXT"),
            ("risk_factors", "TEXT"),
            ("cancellation_reason", "TEXT"),
            ("acceptance_criteria", "TEXT"),
            ("required_tests", "TEXT"),
            ("validation", "TEXT"),
            ("unchanged", "TEXT"),
        ] {
            if !columns.iter().any(|column| column == name) {
                conn.execute_batch(&format!("ALTER TABLE tasks ADD COLUMN {name} {definition}"))?;
            }
        }
        Self::backfill_task_contract_columns(conn)?;
        Self::backfill_task_effort_columns(conn)?;
        Ok(())
    }

    fn backfill_task_effort_columns(conn: &Connection) -> Result<(), DbError> {
        let has_proposals: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'task_proposal_metadata')",
            [],
            |row| row.get(0),
        )?;
        let rows = if has_proposals {
            let mut statement = conn.prepare(
                "SELECT t.id, p.proposal FROM tasks t JOIN task_proposal_metadata p ON p.task_id = t.id WHERE t.reasoning_effort IS NULL OR t.effort_reason IS NULL OR t.risk_factors IS NULL",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let transaction = conn.unchecked_transaction()?;
        for (task_id, proposal_json) in rows {
            let proposal = serde_json::from_str::<crate::protocol::TaskProposal>(&proposal_json)
                .map_err(DbError::Serde)?;
            transaction.execute(
                "UPDATE tasks SET reasoning_effort = COALESCE(reasoning_effort, ?1), effort_reason = COALESCE(effort_reason, ?2), risk_factors = COALESCE(risk_factors, ?3) WHERE id = ?4",
                params![
                    proposal.execution_hints.effort,
                    proposal.execution_hints.effort_reason,
                    serde_json::to_string(&proposal.risk_factors)?,
                    task_id,
                ],
            )?;
        }
        transaction.execute(
            "UPDATE tasks SET reasoning_effort = COALESCE(reasoning_effort, ?1), effort_reason = COALESCE(effort_reason, ?2), risk_factors = COALESCE(risk_factors, '[]')",
            params![
                Task::DEFAULT_REASONING_EFFORT.as_str(),
                Task::DEFAULT_EFFORT_REASON,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    fn backfill_task_contract_columns(conn: &Connection) -> Result<(), DbError> {
        let has_proposals: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'task_proposal_metadata')",
            [],
            |row| row.get(0),
        )?;
        let query = if has_proposals {
            "SELECT t.id, t.objective, p.proposal FROM tasks t LEFT JOIN task_proposal_metadata p ON p.task_id = t.id WHERE t.acceptance_criteria IS NULL OR t.required_tests IS NULL OR t.validation IS NULL OR t.unchanged IS NULL"
        } else {
            "SELECT id, objective, NULL FROM tasks WHERE acceptance_criteria IS NULL OR required_tests IS NULL OR validation IS NULL OR unchanged IS NULL"
        };
        let mut statement = conn.prepare(query)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (task_id, objective, proposal_json) in rows {
            let contract = proposal_json
                .map(|value| {
                    serde_json::from_str::<crate::protocol::TaskProposal>(&value)
                        .map(|proposal| crate::task::TaskContract {
                            unchanged: proposal.unchanged,
                            acceptance_criteria: proposal.acceptance_criteria,
                            required_tests: proposal.required_tests,
                            validation: proposal.validation,
                        })
                        .map_err(DbError::Serde)
                })
                .transpose()?
                .unwrap_or_else(|| crate::task::TaskContract::defaults(&objective));
            conn.execute(
                "UPDATE tasks SET acceptance_criteria = COALESCE(acceptance_criteria, ?1), required_tests = COALESCE(required_tests, ?2), validation = COALESCE(validation, ?3), unchanged = COALESCE(unchanged, ?4) WHERE id = ?5",
                params![
                    serde_json::to_string(&contract.acceptance_criteria)?,
                    serde_json::to_string(&contract.required_tests)?,
                    serde_json::to_string(&contract.validation)?,
                    serde_json::to_string(&contract.unchanged)?,
                    task_id,
                ],
            )?;
        }
        Ok(())
    }

    fn ensure_task_execution_conditions_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_execution_conditions (
                task_id TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,
                details TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            )",
        )?;
        Ok(())
    }

    fn ensure_agent_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(agents)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        for (name, definition) in [
            ("model_version", "INTEGER NOT NULL DEFAULT 1"),
            ("scope", "TEXT NOT NULL DEFAULT 'global'"),
            ("quota_remaining_percent", "INTEGER"),
            ("quota_reset_at", "TEXT"),
            ("quota_checked_at", "TEXT"),
            ("quota_source", "TEXT"),
            ("quota_limits", "TEXT"),
            ("execution_mode", "TEXT NOT NULL DEFAULT 'automated'"),
            ("model", "TEXT"),
            ("reasoning_effort", "TEXT"),
        ] {
            if !columns.iter().any(|column| column == name) {
                conn.execute_batch(&format!(
                    "ALTER TABLE agents ADD COLUMN {name} {definition}"
                ))?;
            }
        }
        Ok(())
    }

    fn ensure_agent_run_columns(conn: &Connection) -> Result<(), DbError> {
        let mut statement = conn.prepare("PRAGMA table_info(agent_runs)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "execution_mode") {
            conn.execute_batch(
                "ALTER TABLE agent_runs ADD COLUMN execution_mode TEXT NOT NULL DEFAULT 'automated'",
            )?;
        }
        if !columns.iter().any(|column| column == "resolved_profile") {
            conn.execute(
                "ALTER TABLE agent_runs ADD COLUMN resolved_profile TEXT",
                [],
            )?;
        }
        for (name, definition) in [
            ("phase", "TEXT"),
            ("error", "TEXT"),
            ("last_activity", "TEXT"),
            ("execution_class", "TEXT NOT NULL DEFAULT 'general'"),
            ("resolved_model", "TEXT"),
            ("resolved_reasoning_effort", "TEXT"),
            ("resolution_source", "TEXT NOT NULL DEFAULT 'legacy'"),
            ("review_consumed", "INTEGER NOT NULL DEFAULT 0"),
            ("source_review_run_id", "INTEGER REFERENCES agent_runs(id)"),
        ] {
            if !columns.iter().any(|column| column == name) {
                conn.execute_batch(&format!(
                    "ALTER TABLE agent_runs ADD COLUMN {name} {definition}"
                ))?;
            }
        }

        conn.execute(
            "UPDATE agent_runs
             SET last_activity = COALESCE(finished_at, started_at, CURRENT_TIMESTAMP)
             WHERE last_activity IS NULL",
            [],
        )?;

        Ok(())
    }

    fn ensure_worker_results_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS worker_results (run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE, outcome TEXT NOT NULL, failure_category TEXT, duration_ms INTEGER, metadata TEXT)")?;
        let columns = conn
            .prepare("PRAGMA table_info(worker_results)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        for column in [
            "total_tokens",
            "input_tokens",
            "output_tokens",
            "cached_input_tokens",
        ] {
            if !columns.iter().any(|value| value == column) {
                conn.execute_batch(&format!(
                    "ALTER TABLE worker_results ADD COLUMN {column} INTEGER"
                ))?;
            }
        }
        Ok(())
    }

    fn ensure_worker_protocol_results_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS worker_protocol_results (run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE, prepare TEXT NOT NULL, execution TEXT)")?;
        Ok(())
    }

    fn ensure_provider_invocations_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS provider_invocations (
                id INTEGER PRIMARY KEY,
                parent_run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
                workflow_id INTEGER REFERENCES workflow_runs(id) ON DELETE SET NULL,
                workflow_stage TEXT,
                workflow_version INTEGER,
                purpose TEXT NOT NULL,
                lineage TEXT NOT NULL DEFAULT 'root',
                attempt INTEGER NOT NULL,
                started_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                finished_at TEXT,
                outcome TEXT NOT NULL DEFAULT 'running',
                effort TEXT,
                selected_agent TEXT,
                selected_model TEXT,
                escalation_reason TEXT,
                total_tokens INTEGER,
                input_tokens INTEGER,
                output_tokens INTEGER,
                UNIQUE(parent_run_id, purpose, attempt)
            )",
        )?;
        for (column, definition) in [
            (
                "workflow_id",
                "INTEGER REFERENCES workflow_runs(id) ON DELETE SET NULL",
            ),
            ("workflow_stage", "TEXT"),
            ("workflow_version", "INTEGER"),
            ("lineage", "TEXT NOT NULL DEFAULT 'root'"),
            ("effort", "TEXT"),
            ("selected_agent", "TEXT"),
            ("selected_model", "TEXT"),
            ("escalation_reason", "TEXT"),
            ("total_tokens", "INTEGER"),
            ("input_tokens", "INTEGER"),
            ("output_tokens", "INTEGER"),
            ("cached_input_tokens", "INTEGER"),
        ] {
            let exists: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('provider_invocations') WHERE name='{column}'"), [], |row| row.get(0))?;
            if exists == 0 {
                conn.execute_batch(&format!(
                    "ALTER TABLE provider_invocations ADD COLUMN {column} {definition}"
                ))?;
            }
        }
        Ok(())
    }

    fn ensure_workflow_tables(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS workflow_runs (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                objective TEXT NOT NULL,
                status TEXT NOT NULL,
                stage TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 0,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                updated_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );
            CREATE INDEX IF NOT EXISTS workflow_runs_project_status
                ON workflow_runs(project_id, status, id);
            CREATE TABLE IF NOT EXISTS workflow_transitions (
                id INTEGER PRIMARY KEY,
                workflow_id INTEGER NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
                from_stage TEXT NOT NULL,
                to_stage TEXT NOT NULL,
                from_status TEXT NOT NULL,
                to_status TEXT NOT NULL,
                edge TEXT NOT NULL,
                deterministic INTEGER NOT NULL,
                provider_run_id INTEGER REFERENCES agent_runs(id),
                details TEXT,
                created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)
            );",
        )?;
        Ok(())
    }

    pub fn start_provider_invocation(
        &self,
        parent_run_id: i64,
        purpose: &str,
        attempt: usize,
        effort: Option<ReasoningEffort>,
    ) -> Result<i64, DbError> {
        let phase_limit = match purpose {
            "implementation" | "revision" => 1,
            "completion_repair" => 2,
            "validation_repair" => 3,
            "lead" | "plan" | "review" => 1,
            _ => 0,
        };
        let used: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM provider_invocations WHERE parent_run_id=?1 AND purpose=?2",
            params![parent_run_id, purpose],
            |row| row.get(0),
        )?;
        let invocation_limit = std::env::var("ORC_PROVIDER_INVOCATION_BUDGET")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(6);
        let total_used: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM provider_invocations WHERE parent_run_id=?1",
            [parent_run_id],
            |row| row.get(0),
        )?;
        if total_used >= invocation_limit {
            return Err(DbError::Scheduler(format!(
                "provider invocation budget exhausted ({total_used}/{invocation_limit})"
            )));
        }
        if phase_limit == 0 || used >= phase_limit {
            return Err(DbError::Scheduler(format!(
                "provider invocation budget exhausted for phase '{purpose}'"
            )));
        }
        let workflow_match: Option<(i64, String, i64)> = self
            .conn
            .query_row(
                "SELECT w.id, w.stage, w.version FROM workflow_runs w
                 JOIN agent_runs r ON r.project_id=w.project_id
                 WHERE r.id=?1 AND w.status='running' AND (
                    (?2='lead' AND w.stage IN ('lead','plan_review')) OR
                    (?2='plan' AND w.stage IN ('planner','planner_revision')) OR
                    (?2 IN ('implementation','completion_repair','validation_repair') AND w.stage='dispatch') OR
                    (?2='review' AND w.stage='review') OR
                    (?2 IN ('revision','completion_repair','validation_repair') AND w.stage='revision')
                 ) ORDER BY w.id DESC LIMIT 1",
                params![parent_run_id, purpose],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (workflow_id, workflow_stage, workflow_version) = workflow_match
            .map(|(id, stage, version)| (Some(id), Some(stage), Some(version)))
            .unwrap_or((None, None, None));
        self.conn.execute(
            "INSERT INTO provider_invocations(parent_run_id, workflow_id, workflow_stage, workflow_version, purpose, lineage, attempt, effort, selected_agent, selected_model, escalation_reason)
             SELECT ?1, ?6, ?7, ?8, ?2, ?3, ?4, ?5, agent, resolved_model,
                    CASE WHEN ?4 = 1 THEN 'initial semantic invocation' ELSE 'bounded evidence-backed repair' END
             FROM agent_runs WHERE id = ?1",
            params![parent_run_id, purpose, format!("{purpose}:{attempt}"), attempt, effort.map(|value| value.as_str()), workflow_id, workflow_stage, workflow_version],
        )?;
        if self.conn.changes() != 1 {
            return Err(DbError::Scheduler(format!(
                "provider invocation parent run {parent_run_id} does not exist"
            )));
        }
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_provider_invocation(
        &self,
        id: i64,
        outcome: &str,
        usage: Option<crate::worker::TokenUsage>,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE provider_invocations SET outcome=?1, finished_at=CURRENT_TIMESTAMP, total_tokens=?3, input_tokens=?4, output_tokens=?5, cached_input_tokens=?6 WHERE id=?2 AND outcome='running'",
            params![
                outcome,
                id,
                usage.map(|v| v.total_tokens),
                usage.and_then(|v| v.input_tokens),
                usage.and_then(|v| v.output_tokens),
                usage.and_then(|v| v.cached_input_tokens),
            ],
        )? != 0)
    }

    pub fn provider_invocations(
        &self,
        parent_run_id: i64,
    ) -> Result<Vec<ProviderInvocation>, DbError> {
        let mut statement = self.conn.prepare(
            "SELECT id, parent_run_id, workflow_id, workflow_stage, workflow_version, purpose, lineage, attempt, started_at, finished_at, outcome, effort, selected_agent, selected_model, escalation_reason, total_tokens, input_tokens, output_tokens, cached_input_tokens FROM provider_invocations WHERE parent_run_id=?1 ORDER BY id",
        )?;
        Ok(statement
            .query_map([parent_run_id], |row| {
                Ok(ProviderInvocation {
                    id: row.get(0)?,
                    parent_run_id: row.get(1)?,
                    workflow_id: row.get(2)?,
                    workflow_stage: row.get(3)?,
                    workflow_version: row.get(4)?,
                    purpose: row.get(5)?,
                    lineage: row.get(6)?,
                    attempt: row.get(7)?,
                    started_at: row.get(8)?,
                    finished_at: row.get(9)?,
                    outcome: row.get(10)?,
                    effort: row
                        .get::<_, Option<String>>(11)?
                        .and_then(|v| ReasoningEffort::parse(&v).ok()),
                    selected_agent: row.get(12)?,
                    selected_model: row.get(13)?,
                    escalation_reason: row.get(14)?,
                    total_tokens: row.get(15)?,
                    input_tokens: row.get(16)?,
                    output_tokens: row.get(17)?,
                    cached_input_tokens: row.get(18)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn completed_workflow_provider_run(
        &self,
        workflow_id: i64,
        workflow_stage: &str,
        workflow_version: i64,
        purpose: &str,
    ) -> Result<Option<i64>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT parent_run_id FROM provider_invocations
             WHERE workflow_id=?1 AND workflow_stage=?2 AND workflow_version=?3
               AND purpose=?4 AND outcome='completed'
             ORDER BY id DESC LIMIT 1",
                params![workflow_id, workflow_stage, workflow_version, purpose],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn start_workflow(
        &self,
        project_id: i64,
        objective: &str,
        policy: &crate::workflow::WorkflowPolicy,
    ) -> Result<crate::workflow::WorkflowRun, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [project_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(DbError::ProjectNotFound(project_id));
        }
        let active = {
            let mut statement = tx.prepare(
                "SELECT state FROM workflow_runs WHERE project_id=?1 AND status IN ('running','waiting_user','acceptance_ready','waiting_external') ORDER BY id",
            )?;
            statement
                .query_map([project_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for state in active {
            let mut old: crate::workflow::WorkflowRun = serde_json::from_str(&state)?;
            let from_status = old.status;
            old.status = crate::workflow::WorkflowStatus::Superseded;
            old.stop_reason = Some("superseded by a newer workflow".into());
            old.version += 1;
            old.transition_count += 1;
            old.updated_at = tx.query_row("SELECT CURRENT_TIMESTAMP", [], |row| row.get(0))?;
            tx.execute(
                "UPDATE workflow_runs SET status='superseded', version=?2, state=?3, updated_at=CURRENT_TIMESTAMP WHERE id=?1",
                params![old.id, old.version, serde_json::to_string(&old)?],
            )?;
            tx.execute(
                "INSERT INTO workflow_transitions(workflow_id, from_stage, to_stage, from_status, to_status, edge, deterministic, details)
                 VALUES (?1, ?2, ?2, ?3, 'superseded', 'superseded', 1, ?4)",
                params![old.id, old.stage.as_str(), from_status.as_str(), old.stop_reason],
            )?;
        }
        tx.execute(
            "INSERT INTO workflow_runs(project_id, objective, status, stage, state) VALUES (?1, ?2, 'running', 'discovery', '{}')",
            params![project_id, objective.trim()],
        )?;
        let id = tx.last_insert_rowid();
        let created_at: String = tx.query_row(
            "SELECT created_at FROM workflow_runs WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        let run = crate::workflow::WorkflowRun {
            id,
            project_id,
            objective: objective.trim().to_owned(),
            status: crate::workflow::WorkflowStatus::Running,
            stage: crate::workflow::WorkflowStage::Discovery,
            version: 0,
            policy: policy.clone(),
            transition_count: 0,
            plan_revision_count: 0,
            task_revision_count: 0,
            current_task_id: None,
            lead_decision_id: None,
            plan_id: None,
            provider_run_id: None,
            revision_feedback: None,
            resume_stage: None,
            user_resolution: None,
            discovery_fingerprint: None,
            stop_reason: None,
            created_at: created_at.clone(),
            updated_at: created_at,
        };
        tx.execute(
            "UPDATE workflow_runs SET state=?2 WHERE id=?1",
            params![id, serde_json::to_string(&run)?],
        )?;
        tx.commit()?;
        Ok(run)
    }

    pub fn get_workflow(&self, id: i64) -> Result<Option<crate::workflow::WorkflowRun>, DbError> {
        let state: Option<String> = self
            .conn
            .query_row("SELECT state FROM workflow_runs WHERE id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        state
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(DbError::from)
    }

    pub fn active_workflow(
        &self,
        project_id: i64,
    ) -> Result<Option<crate::workflow::WorkflowRun>, DbError> {
        let state: Option<String> = self.conn.query_row(
            "SELECT state FROM workflow_runs WHERE project_id=?1 AND status IN ('running','waiting_user','acceptance_ready','waiting_external') ORDER BY id DESC LIMIT 1",
            [project_id], |row| row.get(0)).optional()?;
        state
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(DbError::from)
    }

    pub fn commit_workflow_transition(
        &self,
        current: &crate::workflow::WorkflowRun,
        proposed: &crate::workflow::WorkflowRun,
        edge: &str,
        deterministic: bool,
        provider_run_id: Option<i64>,
        details: Option<&str>,
    ) -> Result<crate::workflow::WorkflowRun, DbError> {
        if current.id != proposed.id || current.project_id != proposed.project_id {
            return Err(DbError::Scheduler(
                "workflow transition changed workflow identity".into(),
            ));
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut committed = proposed.clone();
        committed.version = current.version + 1;
        committed.transition_count = current.transition_count + 1;
        committed.updated_at = tx.query_row("SELECT CURRENT_TIMESTAMP", [], |row| row.get(0))?;
        let changed = tx.execute(
            "UPDATE workflow_runs
             SET status=?1, stage=?2, version=?3, state=?4, updated_at=CURRENT_TIMESTAMP
             WHERE id=?5 AND project_id=?6 AND status=?7 AND stage=?8 AND version=?9",
            params![
                committed.status.as_str(),
                committed.stage.as_str(),
                committed.version,
                serde_json::to_string(&committed)?,
                current.id,
                current.project_id,
                current.status.as_str(),
                current.stage.as_str(),
                current.version,
            ],
        )?;
        if changed != 1 {
            return Err(DbError::Scheduler(
                "workflow changed while committing transition; reload and continue".into(),
            ));
        }
        tx.execute(
            "INSERT INTO workflow_transitions(workflow_id, from_stage, to_stage, from_status, to_status, edge, deterministic, provider_run_id, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                current.id,
                current.stage.as_str(),
                committed.stage.as_str(),
                current.status.as_str(),
                committed.status.as_str(),
                edge,
                deterministic,
                provider_run_id,
                details,
            ],
        )?;
        if let Some(provider_run_id) = provider_run_id {
            tx.execute(
                "UPDATE provider_invocations SET workflow_id=?1 WHERE parent_run_id=?2 AND workflow_id IS NULL",
                params![current.id, provider_run_id],
            )?;
        }
        tx.commit()?;
        Ok(committed)
    }

    pub fn workflow_transitions(
        &self,
        workflow_id: i64,
    ) -> Result<Vec<crate::workflow::WorkflowTransitionRecord>, DbError> {
        let mut statement = self.conn.prepare(
            "SELECT id, workflow_id, from_stage, to_stage, from_status, to_status, edge, deterministic, provider_run_id, details, created_at
             FROM workflow_transitions WHERE workflow_id=?1 ORDER BY id",
        )?;
        Ok(statement
            .query_map([workflow_id], |row| {
                Ok(crate::workflow::WorkflowTransitionRecord {
                    id: row.get(0)?,
                    workflow_id: row.get(1)?,
                    from_stage: row.get(2)?,
                    to_stage: row.get(3)?,
                    from_status: row.get(4)?,
                    to_status: row.get(5)?,
                    edge: row.get(6)?,
                    deterministic: row.get(7)?,
                    provider_run_id: row.get(8)?,
                    details: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn store_worker_prepare(
        &self,
        run_id: i64,
        plan: &crate::worker_protocol::WorkerPlan,
    ) -> Result<(), DbError> {
        plan.validate()
            .map_err(|error| DbError::Scheduler(error.to_string()))?;
        let value = serde_json::to_string(plan).map_err(DbError::Serde)?;
        self.conn.execute("INSERT OR REPLACE INTO worker_protocol_results(run_id, prepare, execution) VALUES (?1, ?2, COALESCE((SELECT execution FROM worker_protocol_results WHERE run_id=?1), NULL))", params![run_id, value])?;
        Ok(())
    }

    pub fn store_worker_execution(
        &self,
        run_id: i64,
        result: &crate::worker_protocol::WorkerExecutionResult,
    ) -> Result<(), DbError> {
        let value = serde_json::to_string(result).map_err(DbError::Serde)?;
        let changed = self.conn.execute(
            "UPDATE worker_protocol_results SET execution=?1 WHERE run_id=?2",
            params![value, run_id],
        )?;
        if changed != 1 {
            return Err(DbError::Scheduler(format!(
                "Worker PREPARE result does not exist for run {run_id}"
            )));
        }
        Ok(())
    }

    pub fn worker_protocol_result(
        &self,
        run_id: i64,
    ) -> Result<Option<(String, Option<String>)>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT prepare, execution FROM worker_protocol_results WHERE run_id=?1",
                [run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// Reopen persisted protocol evidence through the same typed boundary used
    /// by execution, so callers cannot mistake an unrelated JSON blob for a
    /// valid plan or execution result.
    pub fn load_worker_protocol(
        &self,
        run_id: i64,
    ) -> Result<
        Option<(
            crate::worker_protocol::WorkerPlan,
            Option<crate::worker_protocol::WorkerExecutionResult>,
        )>,
        DbError,
    > {
        let Some((prepare, execution)) = self.worker_protocol_result(run_id)? else {
            return Ok(None);
        };
        let mut plan: crate::worker_protocol::WorkerPlan =
            serde_json::from_str(&prepare).map_err(DbError::Serde)?;
        plan.upgrade_legacy();
        plan.validate().map_err(|error| {
            DbError::Scheduler(format!("persisted Worker PREPARE is invalid: {error}"))
        })?;
        let execution = execution
            .map(|value| serde_json::from_str(&value).map_err(DbError::Serde))
            .transpose()?;
        Ok(Some((plan, execution)))
    }

    fn ensure_worktree_metadata_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS worktree_metadata (agent_run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE, task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, branch_name TEXT NOT NULL, worktree_path TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP))")?;
        Ok(())
    }

    fn ensure_change_evidence_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS run_change_evidence (run_id INTEGER PRIMARY KEY REFERENCES agent_runs(id) ON DELETE CASCADE, evidence TEXT NOT NULL, captured_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP))")?;
        Ok(())
    }

    fn ensure_review_blockers_table(conn: &Connection) -> Result<(), DbError> {
        let old = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='review_blockers'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .is_some();
        if old {
            let pk = conn
                .prepare("PRAGMA table_info(review_blockers)")?
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(5)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            if pk.iter().filter(|(_, p)| *p > 0).count() == 2 {
                conn.execute_batch(
                    "ALTER TABLE review_blockers RENAME TO review_blocker_ledger_legacy",
                )?;
            }
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS review_blocker_ledger (task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, blocker_id TEXT NOT NULL, run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE, blocker_key TEXT NOT NULL DEFAULT '', requirement_ref TEXT NOT NULL, evidence TEXT NOT NULL, severity TEXT NOT NULL, acceptance_condition TEXT NOT NULL, status TEXT NOT NULL, finding TEXT NOT NULL, first_seen TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), last_seen TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), updated_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), PRIMARY KEY(task_id, blocker_id)); CREATE TABLE IF NOT EXISTS review_blocker_observations (task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, blocker_id TEXT NOT NULL, run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE, blocker_key TEXT NOT NULL DEFAULT '', requirement_ref TEXT NOT NULL, evidence TEXT NOT NULL, severity TEXT NOT NULL, acceptance_condition TEXT NOT NULL, status TEXT NOT NULL, finding TEXT NOT NULL, first_seen TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), last_seen TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), PRIMARY KEY(run_id, blocker_id));")?;
        if conn.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='review_blocker_ledger_legacy'", [], |_| Ok(true)).optional()?.is_some() {
            conn.execute_batch("INSERT OR IGNORE INTO review_blocker_ledger SELECT task_id, blocker_id, run_id, '', requirement_ref, evidence, severity, acceptance_condition, status, finding, first_seen, last_seen, updated_at FROM review_blocker_ledger_legacy; INSERT OR IGNORE INTO review_blocker_observations SELECT task_id, blocker_id, run_id, '', requirement_ref, evidence, severity, acceptance_condition, status, finding, first_seen, last_seen FROM review_blocker_ledger_legacy;")?;
        }
        Ok(())
    }

    fn ensure_revision_contracts_table(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS revision_contracts (id INTEGER PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, source_review_run_id INTEGER NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE, contract TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'actionable', created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), consumed_at TEXT, UNIQUE(source_review_run_id)); CREATE INDEX IF NOT EXISTS idx_revision_contracts_actionable ON revision_contracts(task_id, status, id);")?;
        Ok(())
    }

    pub fn persist_revision_contract(
        &self,
        task_id: &str,
        review_run_id: i64,
        contract: &str,
    ) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE revision_contracts SET status='superseded' WHERE task_id=?1 AND status='actionable'", [task_id])?;
        tx.execute("INSERT OR REPLACE INTO revision_contracts(task_id, source_review_run_id, contract, status) VALUES (?1, ?2, ?3, 'actionable')", params![task_id, review_run_id, contract])?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_actionable_revision_contracts(&self, task_id: &str) -> Result<(), DbError> {
        self.conn.execute("UPDATE revision_contracts SET status='superseded' WHERE task_id=?1 AND status='actionable'", [task_id])?;
        Ok(())
    }

    pub fn actionable_revision_contract(
        &self,
        task_id: &str,
    ) -> Result<Option<(i64, String, i64)>, DbError> {
        Ok(self.conn.query_row("SELECT source_review_run_id, contract, id FROM revision_contracts WHERE task_id=?1 AND status='actionable' ORDER BY id DESC LIMIT 1", [task_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?)
    }

    /// Count persisted contracts, including superseded and consumed history.
    pub fn revision_contract_history_count(&self, task_id: &str) -> Result<i64, DbError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM revision_contracts WHERE task_id=?1",
            [task_id],
            |row| row.get(0),
        )?)
    }

    pub fn consume_revision_contract(&self, id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute("UPDATE revision_contracts SET status='consumed', consumed_at=CURRENT_TIMESTAMP WHERE id=?1 AND status='actionable'", [id])? != 0)
    }

    pub fn review_blocker_ledger(
        &self,
        task_id: &str,
    ) -> Result<Vec<ReviewBlockerRecord>, DbError> {
        let mut stmt = self.conn.prepare("SELECT task_id, blocker_id, run_id, requirement_ref, evidence, severity, acceptance_condition, status, finding, first_seen, last_seen, blocker_key FROM review_blocker_ledger WHERE task_id=?1 ORDER BY first_seen, blocker_id")?;
        Ok(stmt
            .query_map([task_id], |r| {
                Ok(ReviewBlockerRecord {
                    task_id: r.get(0)?,
                    blocker_id: r.get(1)?,
                    run_id: r.get(2)?,
                    requirement_ref: r.get(3)?,
                    evidence: r.get(4)?,
                    severity: r.get(5)?,
                    acceptance_condition: r.get(6)?,
                    status: r.get(7)?,
                    finding: r.get(8)?,
                    first_seen: r.get(9)?,
                    last_seen: r.get(10)?,
                    blocker_key: r.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn review_blocker_observations(
        &self,
        run_id: i64,
    ) -> Result<Vec<ReviewBlockerRecord>, DbError> {
        let mut stmt = self.conn.prepare("SELECT task_id, blocker_id, run_id, requirement_ref, evidence, severity, acceptance_condition, status, finding, first_seen, last_seen, blocker_key FROM review_blocker_observations WHERE run_id=?1 ORDER BY blocker_id")?;
        Ok(stmt
            .query_map([run_id], |r| {
                Ok(ReviewBlockerRecord {
                    task_id: r.get(0)?,
                    blocker_id: r.get(1)?,
                    run_id: r.get(2)?,
                    requirement_ref: r.get(3)?,
                    evidence: r.get(4)?,
                    severity: r.get(5)?,
                    acceptance_condition: r.get(6)?,
                    status: r.get(7)?,
                    finding: r.get(8)?,
                    first_seen: r.get(9)?,
                    last_seen: r.get(10)?,
                    blocker_key: r.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn store_review_blockers(
        &self,
        task_id: &str,
        run_id: i64,
        blockers: &[crate::automated::ReviewBlocker],
    ) -> Result<(), DbError> {
        for blocker in blockers {
            self.conn.execute("INSERT OR IGNORE INTO review_blocker_observations (task_id, blocker_id, run_id, blocker_key, requirement_ref, evidence, severity, acceptance_condition, status, finding) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![task_id, blocker.id, run_id, blocker.blocker_key, blocker.requirement_ref, blocker.evidence, blocker.severity, blocker.acceptance_condition, blocker.status, blocker.finding])?;
            self.conn.execute("INSERT INTO review_blocker_ledger (task_id, blocker_id, run_id, blocker_key, requirement_ref, evidence, severity, acceptance_condition, status, finding) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(task_id, blocker_id) DO UPDATE SET run_id=excluded.run_id, blocker_key=excluded.blocker_key, status=excluded.status, evidence=excluded.evidence, requirement_ref=excluded.requirement_ref, acceptance_condition=excluded.acceptance_condition, finding=excluded.finding, last_seen=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP", params![task_id, blocker.id, run_id, blocker.blocker_key, blocker.requirement_ref, blocker.evidence, blocker.severity, blocker.acceptance_condition, blocker.status, blocker.finding])?;
        }
        Ok(())
    }

    /// Atomically publishes a validated task review. Until this transaction
    /// commits, neither its blocker ledger changes nor its verdict can affect
    /// revision actionability.
    #[expect(
        clippy::too_many_arguments,
        reason = "the transaction boundary needs the complete validated review payload"
    )]
    pub fn commit_task_review_result(
        &self,
        task_id: &str,
        run_id: i64,
        blockers: &[crate::automated::ReviewBlocker],
        revision_contract: Option<&str>,
        supersedes_with_pass: bool,
        output: &str,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        for blocker in blockers {
            tx.execute("INSERT OR IGNORE INTO review_blocker_observations (task_id, blocker_id, run_id, blocker_key, requirement_ref, evidence, severity, acceptance_condition, status, finding) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", params![task_id, blocker.id, run_id, blocker.blocker_key, blocker.requirement_ref, blocker.evidence, blocker.severity, blocker.acceptance_condition, blocker.status, blocker.finding])?;
            tx.execute("INSERT INTO review_blocker_ledger (task_id, blocker_id, run_id, blocker_key, requirement_ref, evidence, severity, acceptance_condition, status, finding) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(task_id, blocker_id) DO UPDATE SET run_id=excluded.run_id, blocker_key=excluded.blocker_key, status=excluded.status, evidence=excluded.evidence, requirement_ref=excluded.requirement_ref, acceptance_condition=excluded.acceptance_condition, finding=excluded.finding, last_seen=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP", params![task_id, blocker.id, run_id, blocker.blocker_key, blocker.requirement_ref, blocker.evidence, blocker.severity, blocker.acceptance_condition, blocker.status, blocker.finding])?;
        }
        if revision_contract.is_some() || supersedes_with_pass {
            tx.execute("UPDATE revision_contracts SET status='superseded' WHERE task_id=?1 AND status='actionable'", [task_id])?;
        }
        if let Some(contract) = revision_contract {
            tx.execute("INSERT OR REPLACE INTO revision_contracts(task_id, source_review_run_id, contract, status) VALUES (?1, ?2, ?3, 'actionable')", params![task_id, run_id, contract])?;
        }
        let changed = tx.execute("UPDATE agent_runs SET status='completed', output=?1, error=NULL, finished_at=CURRENT_TIMESTAMP, last_activity=CURRENT_TIMESTAMP WHERE id=?2 AND status IN ('running','waiting_external')", params![output, run_id])?;
        if changed == 0 {
            return Err(DbError::InvalidRunStatus(run_id));
        }
        tx.execute(
            "DELETE FROM execution_reservations WHERE run_id=?1",
            [run_id],
        )?;
        tx.commit()?;
        self.record_worker_result(run_id, "completed", Some(output), token_usage)?;
        Ok(())
    }

    pub fn store_change_evidence(
        &self,
        run_id: i64,
        changes: &crate::git::WorktreeChanges,
    ) -> Result<(), DbError> {
        let evidence = serde_json::to_string(changes)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO run_change_evidence (run_id, evidence) VALUES (?1, ?2)",
            params![run_id, evidence],
        )?;
        Ok(())
    }

    pub fn get_change_evidence(
        &self,
        run_id: i64,
    ) -> Result<Option<crate::git::WorktreeChanges>, DbError> {
        let payload: Option<String> = self
            .conn
            .query_row(
                "SELECT evidence FROM run_change_evidence WHERE run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(DbError::from)
    }

    fn ensure_lead_tables(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS lead_turns (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE, role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP)); CREATE TABLE IF NOT EXISTS lead_decisions (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE, kind TEXT NOT NULL, proposal TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending', created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), applying_at TEXT, resolved_at TEXT, snapshot TEXT, run_id INTEGER, source_request TEXT NOT NULL DEFAULT '', summary TEXT NOT NULL DEFAULT '', resolution TEXT, superseded_by_id INTEGER REFERENCES lead_decisions(id));")?;
        let has_supersession: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_table_info('lead_decisions') WHERE name='superseded_by_id'", [], |r| r.get(0)).unwrap_or(0);
        if has_supersession == 0 {
            conn.execute("ALTER TABLE lead_decisions ADD COLUMN superseded_by_id INTEGER REFERENCES lead_decisions(id)", [])?;
        }
        let columns = conn
            .prepare("PRAGMA table_info(lead_decisions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "status") {
            conn.execute(
                "ALTER TABLE lead_decisions ADD COLUMN status TEXT NOT NULL DEFAULT 'pending'",
                [],
            )?;
        }
        if !columns.iter().any(|column| column == "resolved_at") {
            conn.execute("ALTER TABLE lead_decisions ADD COLUMN resolved_at TEXT", [])?;
        }
        if !columns.iter().any(|column| column == "applying_at") {
            conn.execute("ALTER TABLE lead_decisions ADD COLUMN applying_at TEXT", [])?;
        }
        if !columns.iter().any(|column| column == "snapshot") {
            conn.execute("ALTER TABLE lead_decisions ADD COLUMN snapshot TEXT", [])?;
        }
        if !columns.iter().any(|column| column == "run_id") {
            conn.execute("ALTER TABLE lead_decisions ADD COLUMN run_id INTEGER", [])?;
        }
        if !columns.iter().any(|column| column == "source_request") {
            conn.execute(
                "ALTER TABLE lead_decisions ADD COLUMN source_request TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !columns.iter().any(|column| column == "summary") {
            conn.execute(
                "ALTER TABLE lead_decisions ADD COLUMN summary TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !columns.iter().any(|column| column == "resolution") {
            conn.execute("ALTER TABLE lead_decisions ADD COLUMN resolution TEXT", [])?;
        }
        let legacy = {
            let mut statement = conn.prepare("SELECT id, kind, proposal FROM lead_decisions")?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (id, kind, value) in legacy {
            if !matches!(
                kind.as_str(),
                "DIRECT_TASKS" | "PLAN_REQUIRED" | "USER_DECISION_REQUIRED"
            ) && serde_json::from_str::<crate::lead::LeadProposalKind>(&value).is_err()
            {
                let proposal = crate::lead::LeadProposalKind::ApprovalRequest {
                    reason: format!("Migrated legacy Lead proposal ({kind})"),
                    details: value,
                };
                conn.execute(
                    "UPDATE lead_decisions SET kind = 'approval_request', proposal = ?1 WHERE id = ?2",
                    params![serde_json::to_string(&proposal)?, id],
                )?;
            }
        }
        Ok(())
    }

    fn ensure_plan_tables(conn: &Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS plans (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE, version INTEGER NOT NULL, parent_plan_id INTEGER REFERENCES plans(id), source_lead_decision_id INTEGER NOT NULL, source_planner_run_id INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'proposed', response TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), superseded_by_plan_id INTEGER REFERENCES plans(id)); CREATE UNIQUE INDEX IF NOT EXISTS plans_project_version ON plans(project_id, version); CREATE TABLE IF NOT EXISTS plan_dependencies (plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE, task_local_id TEXT NOT NULL, depends_on_local_id TEXT NOT NULL, PRIMARY KEY(plan_id, task_local_id, depends_on_local_id)); CREATE TABLE IF NOT EXISTS plan_reviews (id INTEGER PRIMARY KEY, plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE, lead_run_id INTEGER NOT NULL REFERENCES agent_runs(id), lead_decision_id INTEGER NOT NULL REFERENCES lead_decisions(id), decision TEXT NOT NULL, details TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), superseded_by_review_id INTEGER REFERENCES plan_reviews(id));",
        )?;
        for (table, column, definition) in [
            (
                "plans",
                "superseded_by_plan_id",
                "INTEGER REFERENCES plans(id)",
            ),
            (
                "plan_reviews",
                "superseded_by_review_id",
                "INTEGER REFERENCES plan_reviews(id)",
            ),
        ] {
            let exists: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name='{column}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            if exists == 0 {
                conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    pub fn record_plan_review(
        &self,
        plan_id: i64,
        lead_run_id: i64,
        decision_id: i64,
        decision: &crate::lead::LeadDecisionKind,
        details: &str,
    ) -> Result<i64, DbError> {
        if !matches!(
            decision,
            crate::lead::LeadDecisionKind::Approve
                | crate::lead::LeadDecisionKind::RevisePlan
                | crate::lead::LeadDecisionKind::UserDecisionRequired
        ) {
            return Err(DbError::Scheduler("invalid plan review decision".into()));
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("INSERT INTO plan_reviews (plan_id, lead_run_id, lead_decision_id, decision, details) VALUES (?1, ?2, ?3, ?4, ?5)", params![plan_id, lead_run_id, decision_id, lead_decision_kind(*decision), details])?;
        let review_id = tx.last_insert_rowid();
        tx.execute("UPDATE plan_reviews SET superseded_by_review_id = ?1 WHERE plan_id = ?2 AND id <> ?1 AND superseded_by_review_id IS NULL", params![review_id, plan_id])?;
        let status = match decision {
            crate::lead::LeadDecisionKind::Approve => "approved",
            crate::lead::LeadDecisionKind::RevisePlan => "revision_requested",
            _ => "under_review",
        };
        tx.execute(
            "UPDATE plans SET status=?1 WHERE id=?2 AND status='proposed'",
            params![status, plan_id],
        )?;
        tx.commit()?;
        Ok(review_id)
    }

    pub fn get_plan_review_for_decision(
        &self,
        decision_id: i64,
    ) -> Result<Option<(i64, String)>, DbError> {
        Ok(self.conn.query_row(
            "SELECT plan_id, details FROM plan_reviews WHERE lead_decision_id = ?1 AND decision = 'REVISE_PLAN' ORDER BY id DESC LIMIT 1",
            [decision_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    /// Persist a revision as the next immutable plan version and consume the
    /// exact actionable review decision in one transaction.
    pub fn store_plan_revision(
        &self,
        project_id: i64,
        decision_id: i64,
        planner_run_id: i64,
        response: &crate::protocol::PlanResponse,
    ) -> Result<(i64, i64), DbError> {
        response
            .validate()
            .map_err(|e| DbError::Scheduler(format!("invalid plan: {e}")))?;
        let tx = self.conn.unchecked_transaction()?;
        let parent: Option<(i64, i64)> = tx.query_row(
            "SELECT r.plan_id, p.version FROM plan_reviews r JOIN plans p ON p.id = r.plan_id JOIN lead_decisions d ON d.id = r.lead_decision_id WHERE r.lead_decision_id = ?1 AND p.project_id = ?2 AND r.decision = 'REVISE_PLAN' AND d.kind = 'REVISE_PLAN' AND d.status = 'pending'",
            params![decision_id, project_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((parent_id, parent_version)) = parent else {
            return Err(DbError::Scheduler(
                "no actionable REVISE_PLAN review found".into(),
            ));
        };
        let run_project: Option<i64> = tx
            .query_row(
                "SELECT project_id FROM agent_runs WHERE id = ?1 AND execution_class = 'plan'",
                [planner_run_id],
                |r| r.get(0),
            )
            .optional()?;
        if run_project != Some(project_id) {
            return Err(DbError::Scheduler(
                "invalid source Planner run linkage".into(),
            ));
        }
        tx.execute(
            "UPDATE plans SET status = 'revision_requested' WHERE id = ?1 AND status = 'proposed'",
            [parent_id],
        )?;
        tx.execute("INSERT INTO plans (project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response) VALUES (?1,?2,?3,?4,?5,'proposed',?6)", params![project_id, parent_version + 1, parent_id, decision_id, planner_run_id, serde_json::to_string(response)?])?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE plans SET status = 'cancelled', superseded_by_plan_id = ?1 WHERE id = ?2 AND superseded_by_plan_id IS NULL",
            params![id, parent_id],
        )?;
        for task in &response.tasks {
            for dependency in &task.depends_on {
                tx.execute("INSERT INTO plan_dependencies (plan_id, task_local_id, depends_on_local_id) VALUES (?1,?2,?3)", params![id, task.local_id, dependency])?;
            }
        }
        let changed = tx.execute("UPDATE lead_decisions SET status='consumed', resolved_at=CURRENT_TIMESTAMP WHERE id=?1 AND project_id=?2 AND kind='REVISE_PLAN' AND status='pending'", params![decision_id, project_id])?;
        if changed != 1 {
            return Err(DbError::Scheduler(
                "revision decision changed while Planner was running".into(),
            ));
        }
        tx.commit()?;
        Ok((id, parent_id))
    }

    pub fn record_lead_decision(
        &self,
        project_id: i64,
        kind: &crate::lead::LeadDecisionKind,
        details: &serde_json::Value,
        metadata: LeadDecisionMetadata<'_>,
    ) -> Result<i64, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let payload = serde_json::to_string(details)?;
        transaction.execute("INSERT INTO lead_decisions (project_id, kind, proposal, snapshot, run_id, source_request, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![project_id, lead_decision_kind(*kind), payload, metadata.snapshot, metadata.run_id, metadata.source_request, metadata.summary])?;
        let id = transaction.last_insert_rowid();
        transaction.execute("UPDATE lead_decisions SET status = 'superseded', resolved_at = CURRENT_TIMESTAMP, superseded_by_id = ?2 WHERE project_id = ?1 AND id <> ?2 AND status = 'pending' AND kind IN ('DIRECT_TASKS', 'PLAN_REQUIRED', 'USER_DECISION_REQUIRED')", params![project_id, id])?;
        transaction.commit()?;
        Ok(id)
    }

    pub fn pending_lead_decision(
        &self,
        project_id: i64,
    ) -> Result<Option<crate::lead::PersistedLeadDecision>, DbError> {
        Ok(self.conn.query_row("SELECT id, kind, proposal, snapshot, status, run_id, created_at, source_request, summary, resolution, resolved_at, superseded_by_id FROM lead_decisions WHERE project_id = ?1 AND status = 'pending' AND kind IN ('DIRECT_TASKS', 'PLAN_REQUIRED', 'USER_DECISION_REQUIRED') ORDER BY id DESC LIMIT 1", params![project_id], |r| {
            let status: String = r.get(4)?;
            Ok(crate::lead::PersistedLeadDecision { id: r.get(0)?, run_id: r.get(5)?, created_at: r.get(6)?, source_request: r.get(7)?, summary: r.get(8)?, kind: parse_lead_decision_kind(&r.get::<_, String>(1)?)?, details: r.get(2)?, snapshot: r.get(3)?, actionable: status == "pending", status, resolution: r.get(9)?, resolved_at: r.get(10)?, superseded_by_id: r.get(11)? })
        }).optional()?)
    }

    pub fn resolve_user_decision(
        &self,
        project_id: i64,
        decision_id: i64,
        resolution: &str,
    ) -> Result<crate::lead::PersistedLeadDecision, DbError> {
        if resolution.trim().is_empty() {
            return Err(DbError::Scheduler(
                "USER_DECISION_REQUIRED resolution must not be empty".into(),
            ));
        }
        let changed = self.conn.execute("UPDATE lead_decisions SET status = 'resolved', resolved_at = CURRENT_TIMESTAMP, resolution = ?1 WHERE id = ?2 AND project_id = ?3 AND kind = 'USER_DECISION_REQUIRED' AND status = 'pending'", params![resolution, decision_id, project_id])?;
        if changed != 1 {
            return Err(DbError::Scheduler(
                "USER_DECISION_REQUIRED decision is missing or already resolved".into(),
            ));
        }
        self.conn.query_row("SELECT id, kind, proposal, snapshot, status, run_id, created_at, source_request, summary, resolution, resolved_at, superseded_by_id FROM lead_decisions WHERE id = ?1", [decision_id], |r| {
            let status: String = r.get(4)?;
            Ok(crate::lead::PersistedLeadDecision { id: r.get(0)?, run_id: r.get(5)?, created_at: r.get(6)?, source_request: r.get(7)?, summary: r.get(8)?, kind: parse_lead_decision_kind(&r.get::<_, String>(1)?)?, details: r.get(2)?, snapshot: r.get(3)?, status: status.clone(), actionable: false, resolution: r.get(9)?, resolved_at: r.get(10)?, superseded_by_id: r.get(11)? })
        }).map_err(DbError::from)
    }

    /// Cancel one still-actionable Lead decision without removing its history.
    pub fn cancel_lead_decision(
        &self,
        project_id: i64,
        decision_id: i64,
        reason: Option<&str>,
    ) -> Result<crate::lead::PersistedLeadDecision, DbError> {
        let resolution = reason.unwrap_or("cancelled by operator");
        let tx = self.conn.unchecked_transaction()?;
        let linked_plan: Option<i64> = tx.query_row(
            "SELECT r.plan_id FROM plan_reviews r JOIN plans p ON p.id=r.plan_id WHERE r.lead_decision_id=?1 AND p.project_id=?2 AND p.status IN ('proposed','under_review','revision_requested','approved') ORDER BY r.id DESC LIMIT 1",
            params![decision_id, project_id], |r| r.get(0)).optional()?;
        let changed = tx.execute(
            "UPDATE lead_decisions SET status='cancelled', resolved_at=CURRENT_TIMESTAMP, resolution=?1 WHERE id=?2 AND project_id=?3 AND status='pending' AND kind IN ('DIRECT_TASKS','PLAN_REQUIRED','USER_DECISION_REQUIRED','REVISE_PLAN')",
            params![resolution, decision_id, project_id],
        )?;
        if changed != 1 {
            return Err(DbError::Scheduler(
                "Lead decision is missing or no longer actionable".into(),
            ));
        }
        if let Some(plan_id) = linked_plan {
            tx.execute("UPDATE plans SET status='cancelled' WHERE id=?1 AND project_id=?2 AND status IN ('proposed','under_review','revision_requested','approved')", params![plan_id, project_id])?;
        }
        let result = tx.query_row(
            "SELECT id, kind, proposal, snapshot, status, run_id, created_at, source_request, summary, resolution, resolved_at, superseded_by_id FROM lead_decisions WHERE id=?1",
            [decision_id], |r| {
                let status: String = r.get(4)?;
                Ok(crate::lead::PersistedLeadDecision { id:r.get(0)?, kind:parse_lead_decision_kind(&r.get::<_,String>(1)?)?, details:r.get(2)?, snapshot:r.get(3)?, status:status.clone(), actionable:false, run_id:r.get(5)?, created_at:r.get(6)?, source_request:r.get(7)?, summary:r.get(8)?, resolution:r.get(9)?, resolved_at:r.get(10)?, superseded_by_id:r.get(11)? })
            }).map_err(DbError::from)?;
        tx.commit()?;
        Ok(result)
    }

    /// Cancel a plan-review gate and its linked actionable decision atomically.
    pub fn cancel_plan_review(
        &self,
        project_id: i64,
        review_id: i64,
        reason: Option<&str>,
    ) -> Result<(), DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let linked: Option<(i64, i64)> = tx.query_row(
            "SELECT r.plan_id, r.lead_decision_id FROM plan_reviews r JOIN plans p ON p.id=r.plan_id WHERE r.id=?1 AND p.project_id=?2 AND p.status IN ('proposed','under_review','revision_requested','approved')",
            params![review_id, project_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let Some((plan_id, decision_id)) = linked else {
            return Err(DbError::Scheduler(
                "plan review is missing or no longer actionable".into(),
            ));
        };
        let resolution = reason.unwrap_or("cancelled by operator");
        let changed = tx.execute("UPDATE lead_decisions SET status='cancelled', resolved_at=CURRENT_TIMESTAMP, resolution=?1 WHERE id=?2 AND status='pending'", params![resolution, decision_id])?;
        if changed != 1 {
            return Err(DbError::Scheduler(
                "linked plan-review decision is missing or no longer actionable".into(),
            ));
        }
        tx.execute("UPDATE plans SET status='cancelled' WHERE id=?1 AND project_id=?2 AND status IN ('proposed','under_review','revision_requested','approved')", params![plan_id, project_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Returns pending Lead decisions for read-only consumers such as Planner.
    pub fn pending_lead_decision_context(
        &self,
    ) -> Result<Vec<crate::lead::PersistedLeadDecision>, DbError> {
        let mut statement = self.conn.prepare("SELECT id, kind, proposal, snapshot, status, run_id, created_at, source_request, summary, resolution, resolved_at, superseded_by_id FROM lead_decisions WHERE status = 'pending' AND kind IN ('DIRECT_TASKS', 'PLAN_REQUIRED', 'USER_DECISION_REQUIRED') ORDER BY id")?;
        Ok(statement
            .query_map([], |r| {
                let status: String = r.get(4)?;
                Ok(crate::lead::PersistedLeadDecision {
                    id: r.get(0)?,
                    kind: parse_lead_decision_kind(&r.get::<_, String>(1)?)?,
                    details: r.get(2)?,
                    snapshot: r.get(3)?,
                    status: status.clone(),
                    actionable: status == "pending",
                    run_id: r.get(5)?,
                    created_at: r.get(6)?,
                    source_request: r.get(7)?,
                    summary: r.get(8)?,
                    resolution: r.get(9)?,
                    resolved_at: r.get(10)?,
                    superseded_by_id: r.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_lead_decisions(
        &self,
        project_id: i64,
    ) -> Result<Vec<crate::lead::PersistedLeadDecision>, DbError> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, proposal, snapshot, status, run_id, created_at, source_request, summary, resolution, resolved_at, superseded_by_id FROM lead_decisions
             WHERE project_id = ?1 AND kind IN ('DIRECT_TASKS', 'PLAN_REQUIRED', 'USER_DECISION_REQUIRED')
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([project_id], |r| {
            let status: String = r.get(4)?;
            Ok(crate::lead::PersistedLeadDecision {
                id: r.get(0)?,
                run_id: r.get(5)?,
                created_at: r.get(6)?,
                source_request: r.get(7)?,
                summary: r.get(8)?,
                kind: parse_lead_decision_kind(&r.get::<_, String>(1)?)?,
                details: r.get(2)?,
                snapshot: r.get(3)?,
                actionable: status == "pending",
                status,
                resolution: r.get(9)?,
                superseded_by_id: r.get(11)?,
                resolved_at: r.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
    }

    pub fn consume_pending_lead_decision(
        &self,
        project_id: i64,
    ) -> Result<Option<crate::lead::PersistedLeadDecision>, DbError> {
        let mut decision = self.pending_lead_decision(project_id)?;
        let transaction = self.conn.unchecked_transaction()?;
        let id: Option<i64> = transaction.query_row("SELECT id FROM lead_decisions WHERE project_id = ?1 AND status = 'pending' AND kind IN ('DIRECT_TASKS', 'PLAN_REQUIRED', 'USER_DECISION_REQUIRED') ORDER BY id DESC LIMIT 1", params![project_id], |r| r.get(0)).optional()?;
        let Some(id) = id else {
            transaction.commit()?;
            return Ok(None);
        };
        transaction.execute("UPDATE lead_decisions SET status = 'consumed', resolved_at = CURRENT_TIMESTAMP WHERE id = ?1", [id])?;
        transaction.commit()?;
        if let Some(ref mut decision) = decision {
            decision.status = "consumed".into();
            decision.actionable = false;
        }
        Ok(decision)
    }

    /// Apply the actionable DIRECT_TASKS decision as one database mutation.
    pub fn apply_pending_lead_decision(
        &self,
        project_id: i64,
    ) -> Result<Option<std::collections::BTreeMap<String, String>>, DbError> {
        let decision = self.pending_lead_decision(project_id)?;
        let Some(decision) = decision else {
            return Ok(None);
        };
        if decision.kind != crate::lead::LeadDecisionKind::DirectTasks {
            return Err(DbError::Scheduler(format!(
                "Lead decision is {:?}, not DIRECT_TASKS",
                decision.kind
            )));
        }
        let value: serde_json::Value = serde_json::from_str(&decision.details)?;
        let tasks: Vec<crate::protocol::TaskProposal> = value
            .get("tasks")
            .cloned()
            .ok_or_else(|| DbError::Scheduler("DIRECT_TASKS decision must contain tasks".into()))
            .and_then(|v| {
                serde_json::from_value(v)
                    .map_err(|e| DbError::Scheduler(format!("invalid DIRECT_TASKS proposals: {e}")))
            })?;
        let response = crate::protocol::PlanResponse {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            objective: "Lead direct tasks".into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks,
        };
        response
            .validate()
            .map_err(|e| DbError::Scheduler(e.to_string()))?;
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let mut mapping = std::collections::BTreeMap::new();
            for task in &response.tasks {
                let id = self.allocate_task_id()?;
                self.insert_task_from_proposal(project_id, &id, task)?;
                mapping.insert(task.local_id.clone(), id);
            }
            for task in &response.tasks {
                for dependency in &task.depends_on {
                    self.add_task_dependency(&mapping[&task.local_id], &mapping[dependency])?;
                }
            }
            let changed = self.conn.execute("UPDATE lead_decisions SET status='consumed', resolved_at=CURRENT_TIMESTAMP WHERE id=?1 AND project_id=?2 AND status='pending'", params![decision.id, project_id])?;
            if changed != 1 {
                return Err(DbError::Scheduler(
                    "Lead decision was already consumed".into(),
                ));
            }
            Ok(mapping)
        })();
        match result {
            Ok(mapping) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(Some(mapping))
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn record_lead_turn(
        &self,
        project_id: i64,
        role: crate::lead::LeadRole,
        content: &str,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO lead_turns (project_id, role, content) VALUES (?1, ?2, ?3)",
            params![project_id, serde_json::to_value(role)?.as_str(), content],
        )?;
        Ok(self.conn.last_insert_rowid())
    }
    pub fn list_lead_turns(
        &self,
        project_id: i64,
        limit: usize,
    ) -> Result<Vec<crate::lead::LeadTurn>, DbError> {
        let mut s = self.conn.prepare("SELECT id, role, content, created_at FROM lead_turns WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2")?;
        Ok(s.query_map(params![project_id, limit as i64], |r| {
            Ok(crate::lead::LeadTurn {
                id: r.get(0)?,
                role: serde_json::from_value(serde_json::Value::String(r.get(1)?)).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    },
                )?,
                content: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?)
    }
    pub fn get_lead_turn(
        &self,
        project_id: i64,
        id: i64,
    ) -> Result<Option<crate::lead::LeadTurn>, DbError> {
        Ok(self.conn.query_row("SELECT id, role, content, created_at FROM lead_turns WHERE project_id = ?1 AND id = ?2", params![project_id, id], |r| {
            Ok(crate::lead::LeadTurn { id: r.get(0)?, role: serde_json::from_value(serde_json::Value::String(r.get(1)?)).map_err(|error| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error)))?, content: r.get(2)?, created_at: r.get(3)? })
        }).optional()?)
    }

    pub fn record_lead_proposal(
        &self,
        project_id: i64,
        proposal: &crate::lead::LeadProposalKind,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO lead_decisions (project_id, kind, proposal) VALUES (?1, ?2, ?3)",
            params![
                project_id,
                lead_proposal_kind(proposal),
                serde_json::to_string(proposal)?
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_lead_proposal(
        &self,
        project_id: i64,
        id: i64,
    ) -> Result<Option<crate::lead::LeadProposal>, DbError> {
        Ok(self.conn.query_row("SELECT id, proposal, status, created_at, applying_at, resolved_at FROM lead_decisions WHERE project_id = ?1 AND id = ?2", params![project_id, id], lead_proposal_from_row).optional()?)
    }

    pub fn list_lead_proposals(
        &self,
        project_id: i64,
        limit: usize,
        status: Option<crate::lead::LeadProposalStatus>,
    ) -> Result<Vec<crate::lead::LeadProposal>, DbError> {
        let status = status.map(lead_proposal_status);
        let mut statement = self.conn.prepare("SELECT id, proposal, status, created_at, applying_at, resolved_at FROM lead_decisions WHERE project_id = ?1 AND kind NOT IN ('DIRECT_TASKS', 'PLAN_REQUIRED', 'USER_DECISION_REQUIRED') AND (?2 IS NULL OR status = ?2) ORDER BY id DESC LIMIT ?3")?;
        Ok(statement
            .query_map(
                params![project_id, status, limit.min(i64::MAX as usize) as i64],
                lead_proposal_from_row,
            )?
            .collect::<Result<_, _>>()?)
    }

    pub fn resolve_lead_proposal(
        &self,
        project_id: i64,
        id: i64,
        status: crate::lead::LeadProposalStatus,
    ) -> Result<bool, DbError> {
        if status == crate::lead::LeadProposalStatus::Pending {
            return Ok(false);
        }
        Ok(self.conn.execute("UPDATE lead_decisions SET status = ?1, resolved_at = CURRENT_TIMESTAMP WHERE project_id = ?2 AND id = ?3 AND status = 'pending'", params![lead_proposal_status(status), project_id, id])? != 0)
    }

    pub fn transition_lead_proposal(
        &self,
        project_id: i64,
        id: i64,
        from: crate::lead::LeadProposalStatus,
        to: crate::lead::LeadProposalStatus,
    ) -> Result<bool, DbError> {
        let resolved_at = if matches!(
            to,
            crate::lead::LeadProposalStatus::Applied | crate::lead::LeadProposalStatus::Rejected
        ) {
            "CURRENT_TIMESTAMP"
        } else {
            "NULL"
        };
        let applying_at = match (from, to) {
            (
                crate::lead::LeadProposalStatus::Pending,
                crate::lead::LeadProposalStatus::Applying,
            ) => "CURRENT_TIMESTAMP",
            (_, crate::lead::LeadProposalStatus::Pending) => "NULL",
            _ => "applying_at",
        };
        let sql = format!(
            "UPDATE lead_decisions SET status = ?1, applying_at = {applying_at}, resolved_at = {resolved_at} WHERE project_id = ?2 AND id = ?3 AND status = ?4"
        );
        Ok(self.conn.execute(
            &sql,
            params![
                lead_proposal_status(to),
                project_id,
                id,
                lead_proposal_status(from)
            ],
        )? != 0)
    }

    pub fn create_project(&self, name: &str) -> Result<i64, DbError> {
        self.conn
            .execute("INSERT INTO projects (name) VALUES (?1)", params![name])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_project_name(&self) -> Result<Option<String>, DbError> {
        Ok(self
            .conn
            .query_row("SELECT name FROM projects ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn project_created_at(&self, project_id: i64) -> Result<String, DbError> {
        Ok(self.conn.query_row(
            "SELECT created_at FROM projects WHERE id = ?1",
            [project_id],
            |r| r.get(0),
        )?)
    }

    pub fn task_created_at(&self, task_id: &str) -> Result<String, DbError> {
        Ok(self.conn.query_row(
            "SELECT created_at FROM tasks WHERE id = ?1",
            [task_id],
            |r| r.get(0),
        )?)
    }

    pub fn get_project_id(&self) -> Result<Option<i64>, DbError> {
        Ok(self
            .conn
            .query_row("SELECT id FROM projects ORDER BY id LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?)
    }

    fn project_exists(&self, project_id: i64) -> Result<bool, DbError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
            [project_id],
            |row| row.get(0),
        )?)
    }

    /// Authorize a project to consume a globally owned agent. The reference is
    /// persisted separately from the global agent so project access never
    /// changes global ownership or provider configuration.
    pub fn reference_global_agent(&self, project_id: i64, agent_id: &str) -> Result<bool, DbError> {
        if !self.project_exists(project_id)? {
            return Err(DbError::ProjectNotFound(project_id));
        }
        if self.get_global_agent(agent_id)?.is_none() {
            return Err(DbError::AgentNotFound(agent_id.to_owned()));
        }
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO project_agent_references (project_id, agent_id) VALUES (?1, ?2)",
            params![project_id, agent_id],
        )? != 0)
    }

    pub fn remove_global_agent_reference(
        &self,
        project_id: i64,
        agent_id: &str,
    ) -> Result<bool, DbError> {
        if !self.project_exists(project_id)? {
            return Err(DbError::ProjectNotFound(project_id));
        }
        let active: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE project_id = ?1 AND agent = ?2 AND status IN ('running', 'waiting_external'))",
            params![project_id, agent_id],
            |row| row.get(0),
        )?;
        if active {
            return Err(DbError::AgentHasActiveRun(agent_id.to_owned()));
        }
        Ok(self.conn.execute(
            "DELETE FROM project_agent_references WHERE project_id = ?1 AND agent_id = ?2",
            params![project_id, agent_id],
        )? != 0)
    }

    pub fn list_project_agent_references(
        &self,
        project_id: i64,
    ) -> Result<Vec<ProjectAgentReference>, DbError> {
        if !self.project_exists(project_id)? {
            return Err(DbError::ProjectNotFound(project_id));
        }
        let mut statement = self.conn.prepare(
            "SELECT project_id, agent_id, created_at FROM project_agent_references WHERE project_id = ?1 ORDER BY agent_id",
        )?;
        Ok(statement
            .query_map([project_id], |row| {
                Ok(ProjectAgentReference {
                    project_id: row.get(0)?,
                    agent_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Resolve only an agent explicitly authorized for this project. This is
    /// the project-to-global boundary used by project-aware callers.
    pub fn resolve_project_agent(
        &self,
        project_id: i64,
        agent_id: &str,
    ) -> Result<Option<crate::registry::Agent>, DbError> {
        if !self.project_exists(project_id)? {
            return Err(DbError::ProjectNotFound(project_id));
        }
        let referenced: Option<String> = self
            .conn
            .query_row(
                "SELECT agent_id FROM project_agent_references WHERE project_id = ?1 AND agent_id = ?2",
                params![project_id, agent_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(referenced) = referenced else {
            return Ok(None);
        };
        self.get_global_agent(&referenced)
    }

    pub fn list_project_agents(
        &self,
        project_id: i64,
    ) -> Result<Vec<crate::registry::Agent>, DbError> {
        self.list_project_agent_references(project_id)?
            .into_iter()
            .map(|reference| {
                self.get_global_agent(&reference.agent_id)?
                    .ok_or_else(|| DbError::AgentNotFound(reference.agent_id))
            })
            .collect()
    }

    pub fn insert_agent(&self, agent: &AgentDefinition) -> Result<(), DbError> {
        self.insert_agent_definition(agent)?;
        if let Some(project_id) = self.get_project_id()? {
            self.conn.execute(
                "INSERT OR IGNORE INTO project_agent_references (project_id, agent_id) VALUES (?1, ?2)",
                params![project_id, agent.id],
            )?;
        }
        Ok(())
    }

    fn insert_agent_definition(&self, agent: &AgentDefinition) -> Result<(), DbError> {
        let capabilities = crate::registry::normalize_capability_names(&agent.capabilities);
        self.registry.execute(
            "INSERT INTO agents (id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                agent.id,
                agent.backend,
                agent.display_name,
                agent.enabled,
                agent.priority,
                serde_json::to_string(&capabilities)?,
                agent.status,
                agent.unavailable_reason,
                agent.profile_path,
                agent.model,
                agent.reasoning_effort.map(ReasoningEffort::as_str),
                agent.config_metadata,
                agent.execution_mode,
                agent.quota_remaining_percent,
                agent.quota_reset_at,
                agent.quota_checked_at,
                agent.quota_source,
                agent.quota_limits.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        for action in &agent.actions {
            self.set_agent_action_profile(
                &agent.id,
                *action,
                agent.model.as_deref(),
                agent.reasoning_effort,
            )?;
        }
        self.registry.execute(
            "INSERT OR IGNORE INTO agent_authorizations (agent_id) VALUES (?1)",
            [&agent.id],
        )?;
        Ok(())
    }

    /// Persist the canonical versioned Agent contract. Agents are intentionally
    /// not associated with a project; they are reusable global identities.
    pub fn insert_global_agent(&self, agent: &crate::registry::Agent) -> Result<(), DbError> {
        if agent.model_version != crate::registry::AGENT_MODEL_VERSION {
            return Err(DbError::Scheduler(format!(
                "unsupported agent model version {}",
                agent.model_version
            )));
        }
        if !agent.is_global() {
            return Err(DbError::Scheduler(
                "only globally owned agents can be registered".into(),
            ));
        }
        self.insert_agent_definition(&agent.to_definition())?;
        self.registry.execute(
            "UPDATE agents SET model_version = ?1, scope = ?2 WHERE id = ?3",
            rusqlite::params![agent.model_version, agent.scope, agent.id],
        )?;
        Ok(())
    }

    /// Atomically replace the globally owned agent configuration, its Orc role
    /// assignments, and its operator authorization evidence.
    pub fn upsert_global_agent_configuration(
        &self,
        agent: &crate::registry::Agent,
        permissions: &[crate::registry::OperatorPermission],
        authorization: &AgentAuthorization,
    ) -> Result<(), DbError> {
        if agent.model_version != crate::registry::AGENT_MODEL_VERSION {
            return Err(DbError::Scheduler(format!(
                "unsupported agent model version {}",
                agent.model_version
            )));
        }
        if !agent.is_global() {
            return Err(DbError::Scheduler(
                "only globally owned agents can be registered".into(),
            ));
        }
        let definition = agent.to_definition();
        let transaction = self.registry.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO agents (id, model_version, scope, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
             ON CONFLICT(id) DO UPDATE SET model_version=excluded.model_version, scope=excluded.scope, backend=excluded.backend, display_name=excluded.display_name, enabled=excluded.enabled, priority=excluded.priority, capabilities=excluded.capabilities, status=excluded.status, unavailable_reason=excluded.unavailable_reason, profile_path=excluded.profile_path, model=excluded.model, reasoning_effort=excluded.reasoning_effort, config_metadata=excluded.config_metadata, execution_mode=excluded.execution_mode, quota_remaining_percent=excluded.quota_remaining_percent, quota_reset_at=excluded.quota_reset_at, quota_checked_at=excluded.quota_checked_at, quota_source=excluded.quota_source, quota_limits=excluded.quota_limits",
            params![
                definition.id,
                agent.model_version,
                agent.scope,
                definition.backend,
                definition.display_name,
                definition.enabled,
                definition.priority,
                serde_json::to_string(&definition.capabilities)?,
                definition.status,
                definition.unavailable_reason,
                definition.profile_path,
                definition.model,
                definition.reasoning_effort.map(ReasoningEffort::as_str),
                definition.config_metadata,
                definition.execution_mode,
                definition.quota_remaining_percent,
                definition.quota_reset_at,
                definition.quota_checked_at,
                definition.quota_source,
                definition.quota_limits.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        transaction.execute(
            "DELETE FROM agent_action_profiles WHERE agent_id = ?1",
            [&definition.id],
        )?;
        for action in &definition.actions {
            transaction.execute(
                "INSERT INTO agent_action_profiles(agent_id, action, model, reasoning_effort) VALUES (?1, ?2, ?3, ?4)",
                params![definition.id, action.as_str(), definition.model, definition.reasoning_effort.map(ReasoningEffort::as_str)],
            )?;
        }
        transaction.execute(
            "INSERT INTO agent_authorizations (agent_id, permissions, authenticated, authentication_method, authentication_detail, verified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, CURRENT_TIMESTAMP)
             ON CONFLICT(agent_id) DO UPDATE SET permissions=excluded.permissions, authenticated=excluded.authenticated, authentication_method=excluded.authentication_method, authentication_detail=excluded.authentication_detail, verified_at=excluded.verified_at",
            params![
                definition.id,
                serde_json::to_string(permissions)?,
                authorization.authenticated,
                authorization.authentication_method,
                authorization.authentication_detail,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn agent_authorization(&self, id: &str) -> Result<Option<AgentAuthorization>, DbError> {
        self.registry
            .query_row(
                "SELECT authenticated, authentication_method, authentication_detail FROM agent_authorizations WHERE agent_id = ?1",
                [id],
                |row| {
                    Ok(AgentAuthorization {
                        authenticated: row.get::<_, i64>(0)? != 0,
                        authentication_method: row.get(1)?,
                        authentication_detail: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
    }

    pub fn agent_permissions(
        &self,
        id: &str,
    ) -> Result<Vec<crate::registry::OperatorPermission>, DbError> {
        let permissions: Option<String> = self
            .registry
            .query_row(
                "SELECT permissions FROM agent_authorizations WHERE agent_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        permissions
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map(|value| value.unwrap_or_default())
            .map_err(DbError::from)
    }

    pub fn set_agent_permissions(
        &self,
        id: &str,
        permissions: &[crate::registry::OperatorPermission],
    ) -> Result<bool, DbError> {
        let changed = self.registry.execute(
            "UPDATE agent_authorizations SET permissions = ?1 WHERE agent_id = ?2",
            params![serde_json::to_string(permissions)?, id],
        )?;
        Ok(changed != 0)
    }

    pub fn get_global_agent(&self, id: &str) -> Result<Option<crate::registry::Agent>, DbError> {
        let definition = self.get_agent(id)?;
        let Some(definition) = definition else {
            return Ok(None);
        };
        let (version, scope): (u16, String) = self.registry.query_row(
            "SELECT model_version, scope FROM agents WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if version != crate::registry::AGENT_MODEL_VERSION
            || scope != crate::registry::GLOBAL_AGENT_SCOPE
        {
            return Err(DbError::Scheduler(format!(
                "agent '{}' has unsupported model version or ownership scope",
                id
            )));
        }
        Ok(Some(
            crate::registry::Agent::from_definition(&definition)
                .map_err(|error| DbError::Scheduler(error.to_string()))?,
        ))
    }

    pub fn list_global_agents(&self) -> Result<Vec<crate::registry::Agent>, DbError> {
        self.list_agents()?
            .into_iter()
            .map(|definition| {
                self.get_global_agent(&definition.id)?.ok_or_else(|| {
                    DbError::Scheduler(format!(
                        "agent '{}' disappeared while listing",
                        definition.id
                    ))
                })
            })
            .collect()
    }

    fn agent_from_row(&self, row: &Row<'_>) -> rusqlite::Result<AgentDefinition> {
        let capabilities_json: String = row.get(5)?;
        let capabilities: Vec<String> =
            serde_json::from_str(&capabilities_json).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "invalid agent capabilities: {error}"
                ))
            })?;
        let capabilities = crate::registry::normalize_capability_names(&capabilities);
        let quota_limits_json: Option<String> = row.get(17)?;
        let quota_limits = quota_limits_json
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!("invalid quota limits: {error}"))
            })?;
        let mut agent = AgentDefinition {
            id: row.get(0)?,
            backend: row.get(1)?,
            execution_mode: row.get(12)?,
            display_name: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
            priority: row.get(4)?,
            capabilities,
            status: row.get(6)?,
            unavailable_reason: row.get(7)?,
            profile_path: row.get(8)?,
            model: row.get(9)?,
            reasoning_effort: row
                .get::<_, Option<String>>(10)?
                .map(|value| ReasoningEffort::parse(&value))
                .transpose()
                .map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?,
            config_metadata: row.get(11)?,
            quota_remaining_percent: row.get(13)?,
            quota_reset_at: row.get(14)?,
            quota_checked_at: row.get(15)?,
            quota_source: row.get(16)?,
            quota_limits,
            actions: Vec::new(),
        };
        agent.actions = self
            .agent_action_profiles(&agent.id)
            .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?
            .into_iter()
            .map(|p| p.action)
            .collect();
        if agent.actions.is_empty() {
            agent.actions = if agent.execution_mode == crate::registry::MANUAL {
                vec![
                    AgentAction::Code,
                    AgentAction::Review,
                    AgentAction::Plan,
                    AgentAction::Lead,
                ]
            } else {
                vec![AgentAction::Code]
            };
        }
        Ok(agent)
    }

    pub fn get_agent(&self, id: &str) -> Result<Option<AgentDefinition>, DbError> {
        Ok(self
            .registry
            .query_row(
                "SELECT id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits FROM agents WHERE id = ?1",
                params![id],
                |row| self.agent_from_row(row),
            )
            .optional()?)
    }

    pub fn list_agents(&self) -> Result<Vec<AgentDefinition>, DbError> {
        let mut statement = self.registry.prepare(
            "SELECT id, backend, display_name, enabled, priority, capabilities, status, unavailable_reason, profile_path, model, reasoning_effort, config_metadata, execution_mode, quota_remaining_percent, quota_reset_at, quota_checked_at, quota_source, quota_limits FROM agents ORDER BY id",
        )?;
        Ok(statement
            .query_map([], |row| self.agent_from_row(row))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Resolve the current project's references against the authoritative
    /// registry at read time. Scheduling and semantic actions use this view;
    /// registry administration continues to use `list_agents`.
    pub fn list_schedulable_agents(&self) -> Result<Vec<AgentDefinition>, DbError> {
        let Some(project_id) = self.get_project_id()? else {
            return Ok(Vec::new());
        };
        self.list_project_agent_references(project_id)?
            .into_iter()
            .map(|reference| {
                self.get_agent(&reference.agent_id)?
                    .ok_or(DbError::AgentNotFound(reference.agent_id))
            })
            .collect()
    }

    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET enabled = ?1 WHERE id = ?2",
            params![enabled, id],
        )? != 0)
    }

    pub fn archive_agent(&self, id: &str) -> Result<(), DbError> {
        let status: Option<String> = self
            .registry
            .query_row(
                "SELECT status FROM agents WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(status) = status else {
            return Err(rusqlite::Error::QueryReturnedNoRows.into());
        };
        if status == "archived" {
            return Err(DbError::AgentAlreadyArchived(id.to_owned()));
        }
        let active: i64 = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE agent = ?1 AND status IN ('running', 'waiting_external'))",
            params![id],
            |row| row.get(0),
        )?;
        if active != 0 {
            return Err(DbError::AgentHasActiveRun(id.to_owned()));
        }
        self.registry.execute(
            "UPDATE agents SET status = 'archived', enabled = 0, unavailable_reason = 'archived' WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn purge_agent(&self, id: &str) -> Result<(), DbError> {
        let exists: bool = self.registry.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(DbError::AgentNotFound(id.to_owned()));
        }
        let active: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE agent = ?1 AND status IN ('running', 'waiting_external'))", [id], |r| r.get(0))?;
        if active {
            return Err(DbError::AgentPurgeActiveRun(id.to_owned()));
        }
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            self.conn
                .execute("DELETE FROM lead_provider_config WHERE agent_id = ?1", [id])?;
            self.conn.execute(
                "DELETE FROM project_agent_references WHERE agent_id = ?1",
                [id],
            )?;
            self.registry
                .execute("DELETE FROM agents WHERE id = ?1", [id])?;
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        result
    }

    pub fn set_agent_priority(&self, id: &str, priority: i64) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET priority = ?1 WHERE id = ?2",
            params![priority, id],
        )? != 0)
    }

    pub fn set_agent_profile_path(&self, id: &str, profile_path: &str) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET profile_path = ?1 WHERE id = ?2",
            params![profile_path, id],
        )? != 0)
    }

    pub fn set_agent_model(&self, id: &str, model: &str) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET model = ?1 WHERE id = ?2",
            params![model, id],
        )? != 0)
    }

    pub fn set_agent_reasoning_effort(
        &self,
        id: &str,
        effort: ReasoningEffort,
    ) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET reasoning_effort = ?1 WHERE id = ?2",
            params![effort.as_str(), id],
        )? != 0)
    }

    pub fn set_agent_execution_mode(
        &self,
        id: &str,
        execution_mode: &str,
    ) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET execution_mode = ?1 WHERE id = ?2",
            params![execution_mode, id],
        )? != 0)
    }

    pub fn set_agent_quota(
        &self,
        id: &str,
        remaining_percent: i64,
        reset_at: Option<&str>,
    ) -> Result<bool, DbError> {
        if !(0..=100).contains(&remaining_percent) {
            return Err(DbError::InvalidQuota(remaining_percent));
        }
        Ok(self.registry.execute(
            "UPDATE agents SET quota_remaining_percent = ?1, quota_reset_at = ?2, quota_checked_at = CURRENT_TIMESTAMP, quota_source = 'manual', quota_limits = NULL WHERE id = ?3",
            params![remaining_percent, reset_at, id],
        )? != 0)
    }

    pub fn quota_reserve(&self) -> Result<i64, DbError> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'quota_reserve'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| {
                value
                    .parse::<i64>()
                    .map_err(|_| rusqlite::Error::InvalidQuery)
            })
            .transpose()
            .map(|value| value.unwrap_or(0))
            .map_err(DbError::from)
    }

    pub fn set_quota_reserve(&self, reserve: i64) -> Result<(), DbError> {
        if !(0..=100).contains(&reserve) {
            return Err(DbError::InvalidQuota(reserve));
        }
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('quota_reserve', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![reserve.to_string()],
        )?;
        Ok(())
    }

    pub fn set_agent_synced_quota(
        &self,
        id: &str,
        remaining_percent: i64,
        reset_at: Option<&str>,
        source: &str,
        limits: &QuotaLimits,
    ) -> Result<bool, DbError> {
        if !(0..=100).contains(&remaining_percent) {
            return Err(DbError::InvalidQuota(remaining_percent));
        }
        Ok(self.registry.execute(
            "UPDATE agents SET quota_remaining_percent = ?1, quota_reset_at = ?2, quota_checked_at = CURRENT_TIMESTAMP, quota_source = ?3, quota_limits = ?4 WHERE id = ?5",
            params![remaining_percent, reset_at, source, serde_json::to_string(limits)?, id],
        )? != 0)
    }

    pub fn clear_agent_quota(&self, id: &str) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET quota_remaining_percent = NULL, quota_reset_at = NULL, quota_checked_at = NULL, quota_source = NULL, quota_limits = NULL WHERE id = ?1",
            params![id],
        )? != 0)
    }

    pub fn set_agent_availability(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
    ) -> Result<bool, DbError> {
        Ok(self.registry.execute(
            "UPDATE agents SET status = ?1, unavailable_reason = ?2 WHERE id = ?3",
            params![status, reason, id],
        )? != 0)
    }

    pub fn store_discovery_facts(
        &self,
        project_id: i64,
        response: &crate::protocol::ProjectDiscoveryResponse,
    ) -> Result<(), DbError> {
        let facts = [
            ("purpose", response.project.purpose.clone()),
            (
                "languages",
                serde_json::to_string(&response.project.languages)?,
            ),
            (
                "build_commands",
                serde_json::to_string(&response.engineering.build_commands)?,
            ),
            (
                "test_commands",
                serde_json::to_string(&response.engineering.test_commands)?,
            ),
            (
                "modules",
                serde_json::to_string(&response.architecture.modules)?,
            ),
            (
                "boundaries",
                serde_json::to_string(&response.architecture.boundaries)?,
            ),
            (
                "entry_points",
                serde_json::to_string(&response.architecture.entry_points)?,
            ),
            (
                "observed_patterns",
                serde_json::to_string(&response.engineering.observed_patterns)?,
            ),
        ];
        for (key, value) in facts {
            self.conn.execute(
                "INSERT INTO project_facts (project_id, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(project_id, key) DO UPDATE SET value = excluded.value",
                params![project_id, key, value],
            )?;
        }
        Ok(())
    }

    pub fn store_discovery_snapshot(
        &self,
        project_id: i64,
        snapshot: &crate::discovery::ProjectDiscoverySnapshot,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO discovery_snapshots(project_id, fingerprint, snapshot) VALUES (?1, ?2, ?3)
             ON CONFLICT(project_id) DO UPDATE SET fingerprint=excluded.fingerprint, snapshot=excluded.snapshot, created_at=CURRENT_TIMESTAMP",
            params![project_id, snapshot.fingerprint, serde_json::to_string(snapshot)?],
        )?;
        Ok(())
    }

    pub fn load_discovery_snapshot(
        &self,
        project_id: i64,
        fingerprint: &str,
    ) -> Result<Option<crate::discovery::ProjectDiscoverySnapshot>, DbError> {
        self.conn
            .query_row(
                "SELECT snapshot FROM discovery_snapshots WHERE project_id=?1 AND fingerprint=?2",
                params![project_id, fingerprint],
                |row| {
                    let value: String = row.get(0)?;
                    serde_json::from_str(&value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
    }

    fn task_from_row(row: &Row<'_>) -> rusqlite::Result<Task> {
        let priority: String = row.get(4)?;
        let status: String = row.get(5)?;
        let priority_value = match priority.as_str() {
            "low" => TaskPriority::Low,
            "normal" => TaskPriority::Normal,
            "high" => TaskPriority::High,
            "critical" => TaskPriority::Critical,
            _ => {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "invalid priority: {priority}"
                )));
            }
        };
        let status_value = match status.as_str() {
            "backlog" => TaskStatus::Backlog,
            "ready" => TaskStatus::Ready,
            "active" => TaskStatus::Active,
            "review" => TaskStatus::Review,
            "blocked" => TaskStatus::Blocked,
            "done" => TaskStatus::Done,
            "cancelled" => TaskStatus::Cancelled,
            _ => {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "invalid status: {status}"
                )));
            }
        };
        let required_capabilities_json: Option<String> = row.get(6)?;
        let required_capabilities: Vec<String> = match required_capabilities_json {
            Some(json_str) if !json_str.trim().is_empty() => serde_json::from_str(&json_str)
                .map_err(|error| {
                    rusqlite::Error::InvalidParameterName(format!(
                        "invalid task capabilities: {error}"
                    ))
                })?,
            _ => Vec::new(),
        };
        let required_capabilities =
            crate::registry::normalize_capability_names(&required_capabilities);
        let scope_mode = match row.get::<_, Option<String>>(7)? {
            Some(value) => Some(TaskScopeMode::parse(&value).ok_or_else(|| {
                rusqlite::Error::InvalidParameterName(format!("invalid task scope mode: {value}"))
            })?),
            None => None,
        };
        let list = |index| -> Result<Vec<String>, rusqlite::Error> {
            match row.get::<_, Option<String>>(index)? {
                Some(value) => serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::InvalidParameterName(format!("invalid task metadata: {error}"))
                }),
                None => Ok(Vec::new()),
            }
        };
        let reasoning_effort = match row.get::<_, Option<String>>(13)? {
            Some(value) => Some(ReasoningEffort::parse(&value).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!("invalid task effort: {error}"))
            })?),
            None => None,
        };
        Ok(Task {
            id: row.get(0)?,
            title: row.get(1)?,
            objective: row.get(2)?,
            role: row.get(3)?,
            priority: priority_value,
            status: status_value,
            cancellation_reason: row.get(10)?,
            required_capabilities,
            scope_mode,
            context_files: list(8)?,
            expected_changes: list(9)?,
            reasoning_effort,
            effort_reason: row.get(14)?,
            risk_factors: match row.get::<_, Option<String>>(15)? {
                Some(value) => serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::InvalidParameterName(format!("invalid task risks: {error}"))
                })?,
                None => Vec::new(),
            },
        })
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, cancellation_reason, execution_class, execution_model, reasoning_effort, effort_reason, risk_factors FROM tasks ORDER BY created_at",
        )?;
        Ok(stmt
            .query_map([], Self::task_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_tasks_for_project(&self, project_id: i64) -> Result<Vec<Task>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, cancellation_reason, execution_class, execution_model, reasoning_effort, effort_reason, risk_factors FROM tasks WHERE project_id = ?1 ORDER BY created_at",
        )?;
        Ok(stmt
            .query_map(params![project_id], Self::task_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Returns the canonical Lead proposal retained for a created task.
    pub fn get_task_proposal_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<crate::protocol::TaskProposal>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT proposal FROM task_proposal_metadata WHERE task_id = ?1",
                params![task_id],
                |row| {
                    let value: String = row.get(0)?;
                    serde_json::from_str(&value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                },
            )
            .optional()?)
    }

    /// Read the Worker contract from the authoritative Task row. Proposal
    /// metadata is deliberately not a fallback here: it is only an input to
    /// task creation and legacy migration.
    pub fn get_task_contract(
        &self,
        task_id: &str,
    ) -> Result<Option<crate::task::TaskContract>, DbError> {
        let contract: Option<Option<crate::task::TaskContract>> = self
            .conn
            .query_row(
                "SELECT acceptance_criteria, required_tests, validation, unchanged FROM tasks WHERE id = ?1",
                [task_id],
                |row| -> rusqlite::Result<Option<crate::task::TaskContract>> {
                    let values: Vec<Option<String>> = (0..4)
                        .map(|index| row.get::<_, Option<String>>(index))
                        .collect::<Result<Vec<_>, _>>()?;
                    if values.iter().any(Option::is_none) {
                        return Ok(None);
                    }
                    let parse = |index: usize| -> Result<Vec<String>, rusqlite::Error> {
                        serde_json::from_str::<Vec<String>>(
                            values[index].as_deref().unwrap_or_default(),
                        )
                        .map_err(|error| {
                                rusqlite::Error::InvalidParameterName(format!(
                                    "invalid task contract metadata: {error}"
                                ))
                            })
                    };
                    Ok(Some(crate::task::TaskContract {
                        acceptance_criteria: parse(0)?,
                        required_tests: parse(1)?,
                        validation: parse(2)?,
                        unchanged: parse(3)?,
                    }))
                },
            )
            .optional()?;
        Ok(contract.flatten())
    }

    /// Return the complete persisted execution selection contract. These
    /// fields are stored on the Task row, not recovered from proposal history.
    pub fn get_task_execution_hints(
        &self,
        task_id: &str,
    ) -> Result<Option<crate::protocol::ExecutionHints>, DbError> {
        self.conn
            .query_row(
                "SELECT execution_class, execution_model, reasoning_effort, effort_reason FROM tasks WHERE id = ?1",
                [task_id],
                |row| {
                    Ok(crate::protocol::ExecutionHints {
                        class: row.get(0)?,
                        model: row.get(1)?,
                        effort: row.get(2)?,
                        effort_reason: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(DbError::from)
    }

    /// Persist the canonical task contract used by Worker PREPARE.  This is a
    /// separate operation from execution so callers can create a task and
    /// retain the exact Lead proposal that authorized it.
    pub fn set_task_proposal_metadata(
        &self,
        task_id: &str,
        proposal: &crate::protocol::TaskProposal,
    ) -> Result<(), DbError> {
        if proposal.local_id != task_id {
            return Err(DbError::Scheduler(format!(
                "task proposal local_id '{}' does not match task '{}'",
                proposal.local_id, task_id
            )));
        }
        proposal
            .validate()
            .map_err(|error| DbError::Scheduler(error.to_string()))?;
        let changed = self.conn.execute(
            "INSERT INTO task_proposal_metadata (task_id, proposal) VALUES (?1, ?2) ON CONFLICT(task_id) DO UPDATE SET proposal=excluded.proposal",
            params![task_id, serde_json::to_string(proposal)?],
        )?;
        if changed != 1 {
            return Err(DbError::TaskNotFound(task_id.to_owned()));
        }
        let effort =
            proposal.execution_hints.effort.as_deref().ok_or_else(|| {
                DbError::Scheduler("task proposal has no execution effort".into())
            })?;
        self.conn.execute(
            "UPDATE tasks SET reasoning_effort = ?1, effort_reason = ?2, risk_factors = ?3, expected_changes = ?4, acceptance_criteria = ?5, required_tests = ?6, validation = ?7, unchanged = ?8, execution_class = ?9, execution_model = ?10 WHERE id = ?11",
            params![
                effort,
                proposal.execution_hints.effort_reason,
                serde_json::to_string(&proposal.risk_factors)?,
                serde_json::to_string(&proposal.expected_changes)?,
                serde_json::to_string(&proposal.acceptance_criteria)?,
                serde_json::to_string(&proposal.required_tests)?,
                serde_json::to_string(&proposal.validation)?,
                serde_json::to_string(&proposal.unchanged)?,
                proposal.execution_hints.class,
                proposal.execution_hints.model,
                task_id
            ],
        )?;
        Ok(())
    }

    pub fn get_task_execution_condition(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskExecutionCondition>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT task_id, kind, details, created_at FROM task_execution_conditions WHERE task_id = ?1",
                [task_id],
                |row| {
                    Ok(TaskExecutionCondition {
                        task_id: row.get(0)?,
                        kind: row.get(1)?,
                        details: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_task_execution_condition(
        &self,
        task_id: &str,
        kind: &str,
        details: &str,
    ) -> Result<(), DbError> {
        if self.get_task(task_id)?.is_none() {
            return Err(DbError::TaskNotFound(task_id.to_owned()));
        }
        self.conn.execute(
            "INSERT INTO task_execution_conditions (task_id, kind, details) VALUES (?1, ?2, ?3) ON CONFLICT(task_id) DO UPDATE SET kind = excluded.kind, details = excluded.details",
            params![task_id, kind, details],
        )?;
        Ok(())
    }

    pub fn completed_revision_effort_for_blocker(
        &self,
        task_id: &str,
        review_run_id: i64,
        blocker_id: &str,
    ) -> Result<Option<ReasoningEffort>, DbError> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT revision.resolved_reasoning_effort
                 FROM agent_runs revision
                 JOIN review_blocker_observations prior
                   ON prior.run_id = revision.source_review_run_id
                  AND prior.blocker_id = ?3
                 JOIN review_blocker_observations current
                   ON current.run_id = ?2
                  AND current.blocker_id = ?3
                 WHERE revision.task_id = ?1
                   AND revision.source_review_run_id IS NOT NULL
                   AND revision.status = 'completed'
                 ORDER BY revision.id DESC LIMIT 1",
                params![task_id, review_run_id, blocker_id],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| {
                ReasoningEffort::parse(&value).map_err(|error| {
                    DbError::Scheduler(format!("invalid persisted revision effort: {error}"))
                })
            })
            .transpose()
    }

    #[allow(dead_code)]
    pub fn insert_task(
        &self,
        project_id: i64,
        title: &str,
        objective: &str,
        role: &str,
        priority: TaskPriority,
    ) -> Result<String, DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let id = self.allocate_task_id()?;
            let priority_str = priority_string(priority);
            self.conn.execute("INSERT INTO tasks (id, project_id, title, objective, role, priority, status, reasoning_effort, effort_reason, risk_factors) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, '[]')", params![id, project_id, title, objective, role, priority_str, Task::DEFAULT_REASONING_EFFORT.as_str(), Task::DEFAULT_EFFORT_REASON])?;
            Ok(id)
        })();
        match result {
            Ok(id) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(id)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn create_task(
        &self,
        project_id: i64,
        input: &crate::task::CreateTaskInput,
    ) -> Result<String, DbError> {
        if input.title.trim().is_empty() {
            return Err(DbError::Scheduler("task title must not be empty".into()));
        }
        if input.objective.trim().is_empty() {
            return Err(DbError::Scheduler(
                "task objective must not be empty".into(),
            ));
        }
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let id = self.allocate_task_id()?;
            let contract = crate::task::TaskContract::defaults(&input.objective);
            let capabilities =
                crate::registry::normalize_capability_names(&input.required_capabilities);
            self.conn.execute("INSERT INTO tasks (id, project_id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, reasoning_effort, effort_reason, risk_factors, acceptance_criteria, required_tests, validation, unchanged) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, ?9, ?10, ?11, ?12, '[]', ?13, ?14, ?15, ?16)", params![id, project_id, input.title, input.objective, input.role, priority_string(input.priority), serde_json::to_string(&capabilities)?, input.scope_mode.map(|value| value.to_string()), serde_json::to_string(&input.context_files)?, serde_json::to_string(&input.expected_changes)?, Task::DEFAULT_REASONING_EFFORT.as_str(), Task::DEFAULT_EFFORT_REASON, serde_json::to_string(&contract.acceptance_criteria)?, serde_json::to_string(&contract.required_tests)?, serde_json::to_string(&contract.validation)?, serde_json::to_string(&contract.unchanged)?])?;
            for dependency in &input.dependencies {
                self.add_task_dependency(&id, dependency)?;
            }
            self.record_lifecycle_event("task_created", Some(&id), None, None, None)?;
            Ok(id)
        })();
        match result {
            Ok(id) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(id)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn allocate_task_id(&self) -> Result<String, DbError> {
        let value: String = self.conn.query_row(
            "SELECT value FROM meta WHERE key = 'next_task_id'",
            [],
            |r| r.get(0),
        )?;
        let seq = value
            .parse::<u64>()
            .map_err(|_| DbError::InvalidSequence(value.clone()))?;
        self.conn.execute(
            "UPDATE meta SET value = ?1 WHERE key = 'next_task_id'",
            params![(seq + 1).to_string()],
        )?;
        Ok(format!("T-{seq:04}"))
    }

    /// Insert a task with a specific ID (mainly for testing).
    #[allow(dead_code)]
    pub fn insert_task_with_id(
        &self,
        project_id: i64,
        id: &str,
        title: &str,
        objective: &str,
        role: &str,
        priority: TaskPriority,
    ) -> Result<String, DbError> {
        let priority_str = match priority {
            TaskPriority::Low => "low",
            TaskPriority::Normal => "normal",
            TaskPriority::High => "high",
            TaskPriority::Critical => "critical",
        };
        self.conn.execute(
            "INSERT INTO tasks (id, project_id, title, objective, role, priority, status, reasoning_effort, effort_reason, risk_factors) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, '[]')",
            params![
                id,
                project_id,
                title,
                objective,
                role,
                priority_str,
                Task::DEFAULT_REASONING_EFFORT.as_str(),
                Task::DEFAULT_EFFORT_REASON,
            ],
        )?;
        Ok(id.to_string())
    }

    /// Apply an Engineering Lead response atomically.
    /// All actions from the response are applied inside a single SQLite transaction.
    /// If any action fails, the transaction is rolled back and no changes from this
    /// response are persisted.
    pub fn apply_engineering_lead_response(
        &self,
        project_id: i64,
        response: &crate::protocol::EngineeringLeadResponse,
    ) -> Result<(), DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            for action in &response.actions {
                match action {
                    crate::protocol::LeadAction::CreateTask {
                        title,
                        objective,
                        role,
                        priority,
                        scope_mode,
                        context_files,
                        expected_changes,
                    } => {
                        // get seq
                        let value: String = self.conn.query_row(
                            "SELECT value FROM meta WHERE key = 'next_task_id'",
                            [],
                            |r| r.get(0),
                        )?;
                        let seq = value
                            .parse::<u64>()
                            .map_err(|_| DbError::InvalidSequence(value.clone()))?;
                        let id = format!("T-{seq:04}");
                        let priority_str = match priority {
                            TaskPriority::Low => "low",
                            TaskPriority::Normal => "normal",
                            TaskPriority::High => "high",
                            TaskPriority::Critical => "critical",
                        };
                        self.conn.execute(
                            "INSERT INTO tasks (id, project_id, title, objective, role, priority, status, scope_mode, context_files, expected_changes, reasoning_effort, effort_reason, risk_factors) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog', ?7, ?8, ?9, ?10, ?11, '[]')",
                            params![id, project_id, title, objective, role, priority_str, scope_mode.map(|v| v.to_string()), serde_json::to_string(context_files)?, serde_json::to_string(expected_changes)?, Task::DEFAULT_REASONING_EFFORT.as_str(), Task::DEFAULT_EFFORT_REASON],
                        )?;
                        self.conn.execute(
                            "UPDATE meta SET value = ?1 WHERE key = 'next_task_id'",
                            params![(seq + 1).to_string()],
                        )?;
                    }
                    crate::protocol::LeadAction::RequireCtoApproval { reason } => {
                        self.conn.execute(
                            "INSERT INTO approval_requests (project_id, reason) VALUES (?1, ?2)",
                            params![project_id, reason],
                        )?;
                    }
                }
            }
            Ok(())
        })();

        match result {
            Ok(_) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    pub fn get_task(&self, id: &str) -> Result<Option<Task>, DbError> {
        Ok(self
            .conn
            .query_row(
                    "SELECT id, title, objective, role, priority, status, required_capabilities, scope_mode, context_files, expected_changes, cancellation_reason, execution_class, execution_model, reasoning_effort, effort_reason, risk_factors FROM tasks WHERE id = ?1",
                params![id],
                Self::task_from_row,
            )
            .optional()?)
    }

    pub fn set_task_required_capabilities(
        &self,
        id: &str,
        capabilities: &[String],
    ) -> Result<bool, DbError> {
        let json =
            serde_json::to_string(&crate::registry::normalize_capability_names(capabilities))?;
        let changed = self.conn.execute(
            "UPDATE tasks SET required_capabilities = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![json, id],
        )?;
        Ok(changed != 0)
    }

    pub fn set_task_scope(&self, id: &str, scope: TaskScopeMode) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE tasks SET scope_mode = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![scope.to_string(), id],
        )? != 0)
    }

    pub fn set_task_context(&self, id: &str, files: &[String]) -> Result<bool, DbError> {
        self.set_task_metadata(id, "context_files", files)
    }
    pub fn set_task_expected_changes(&self, id: &str, files: &[String]) -> Result<bool, DbError> {
        self.set_task_metadata(id, "expected_changes", files)
    }
    fn set_task_metadata(&self, id: &str, column: &str, files: &[String]) -> Result<bool, DbError> {
        let json = serde_json::to_string(files)?;
        Ok(self.conn.execute(
            &format!(
                "UPDATE tasks SET {column} = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2"
            ),
            params![json, id],
        )? != 0)
    }

    #[allow(dead_code)]
    pub fn update_task_status(&self, id: &str, status: TaskStatus) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE tasks SET status = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2",
            params![status.to_string(), id],
        )?;
        Ok(changed != 0)
    }

    pub fn requeue_task(&self, id: &str, reason: &str) -> Result<(), DbError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            let status: Option<String> = self
                .conn
                .query_row(
                    "SELECT status FROM tasks WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            let status = match status.as_deref() {
                Some(status @ ("active" | "blocked")) => status,
                Some(_) => return Err(DbError::TaskNotActive(id.into())),
                None => return Err(DbError::TaskNotFound(id.into())),
            };
            let active_run_id: Option<i64> = self.conn.query_row(
                "SELECT id FROM agent_runs WHERE task_id = ?1 AND status IN ('running', 'waiting_external') ORDER BY started_at DESC, id DESC LIMIT 1",
                params![id], |row| row.get(0),
            ).optional()?;
            if status == "active" {
                let run_id = active_run_id.ok_or_else(|| DbError::NoRecoverableRun(id.into()))?;
                self.conn.execute(
                    "UPDATE agent_runs SET status = 'failed', output = ?1, finished_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')",
                    params![reason, run_id],
                )?;
            } else {
                if active_run_id.is_some() {
                    return Err(DbError::TaskNotActive(id.into()));
                }
                let failed_run_exists: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE task_id = ?1 AND status IN ('failed', 'cancelled'))",
                    params![id],
                    |row| row.get(0),
                )?;
                if !failed_run_exists {
                    return Err(DbError::NoRecoverableRun(id.into()));
                }
            }
            self.conn.execute(
                "UPDATE tasks SET status = 'backlog', updated_at = CURRENT_TIMESTAMP WHERE id = ?1 AND status IN ('active', 'blocked')",
                params![id],
            )?;
            Ok::<_, DbError>(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                self.record_lifecycle_event("task_requeue", Some(id), None, None, Some(reason))?;
                Ok(())
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub fn cancel_task(&self, id: &str, reason: Option<&str>) -> Result<bool, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE tasks SET status = 'cancelled', cancellation_reason = ?1, updated_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status != 'done' AND status != 'cancelled'",
            params![reason, id],
        )?;
        if changed != 0 {
            transaction.execute(
                "UPDATE agent_runs SET status = 'cancelled', output = COALESCE(?1, 'task cancelled'), finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP WHERE task_id = ?2 AND status IN ('running', 'waiting_external')",
                params![reason, id],
            )?;
            transaction.execute(
                "DELETE FROM execution_reservations WHERE run_id IN (SELECT id FROM agent_runs WHERE task_id = ?1 AND status = 'cancelled')",
                [id],
            )?;
        }
        transaction.commit()?;
        Ok(changed != 0)
    }

    #[allow(dead_code)]
    pub fn insert_decision(
        &self,
        project_id: i64,
        task_id: Option<&str>,
        summary: &str,
    ) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO decisions (project_id, task_id, summary) VALUES (?1, ?2, ?3)",
            params![project_id, task_id, summary],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    #[allow(dead_code)]
    pub fn insert_approval_request(&self, project_id: i64, reason: &str) -> Result<i64, DbError> {
        self.conn.execute(
            "INSERT INTO approval_requests (project_id, reason) VALUES (?1, ?2)",
            params![project_id, reason],
        )?;
        let id = self.conn.last_insert_rowid();
        self.record_lifecycle_event("approval_created", None, None, None, Some(reason))?;
        Ok(id)
    }

    #[allow(dead_code)]
    pub fn list_approval_requests(&self, project_id: i64) -> Result<Vec<ApprovalRequest>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, reason, resolved FROM approval_requests WHERE project_id = ?1 ORDER BY id",
        )?;
        Ok(stmt
            .query_map(params![project_id], |r| {
                Ok(ApprovalRequest {
                    id: r.get(0)?,
                    reason: r.get(1)?,
                    resolved: r.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn resolve_approval_request(&self, project_id: i64, id: i64) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE approval_requests SET resolved = 1 WHERE id = ?1 AND project_id = ?2",
            params![id, project_id],
        )?;
        if changed != 0 {
            self.record_lifecycle_event(
                "approval_resolved",
                None,
                None,
                None,
                Some(&id.to_string()),
            )?;
        }
        Ok(changed != 0)
    }

    fn agent_run_from_row(row: &Row<'_>) -> rusqlite::Result<AgentRun> {
        Ok(AgentRun {
            id: row.get(0)?,
            project_id: row.get(1)?,
            task_id: row.get(2)?,
            agent: row.get(3)?,
            execution_mode: row.get(4)?,
            status: row.get(5)?,
            output: row.get(6)?,
            error: row.get(7)?,
            started_at: row.get(8)?,
            finished_at: row.get(9)?,
            phase: row.get(10)?,
            last_activity: row.get(11)?,
            execution_class: row.get(12)?,
            resolved_model: row.get(13)?,
            resolved_reasoning_effort: match row.get::<_, Option<String>>(14)? {
                Some(value) => Some(ReasoningEffort::parse(&value).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        14,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e.to_string(),
                        )),
                    )
                })?),
                None => None,
            },
            resolution_source: row.get(15)?,
            resolved_profile: row.get(16)?,
        })
    }

    pub fn create_agent_run(
        &self,
        project_id: i64,
        task_id: &str,
        agent: &str,
    ) -> Result<i64, DbError> {
        self.create_agent_run_with_mode(project_id, task_id, agent, "automated")
    }

    pub fn create_agent_run_with_mode(
        &self,
        project_id: i64,
        task_id: &str,
        agent: &str,
        execution_mode: &str,
    ) -> Result<i64, DbError> {
        self.create_agent_run_with_execution(
            project_id,
            task_id,
            agent,
            execution_mode,
            AgentRunExecution {
                class: "general",
                model: None,
                effort: None,
                source: "legacy",
            },
        )
    }

    pub fn create_agent_run_with_execution(
        &self,
        project_id: i64,
        task_id: &str,
        agent: &str,
        execution_mode: &str,
        execution: AgentRunExecution<'_>,
    ) -> Result<i64, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute("INSERT INTO agent_runs (project_id, task_id, agent, execution_mode, execution_class, resolved_model, resolved_reasoning_effort, resolution_source, status, started_at, phase, last_activity) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'running', CURRENT_TIMESTAMP, 'starting', CURRENT_TIMESTAMP)", params![project_id, task_id, agent, execution_mode, execution.class, execution.model, execution.effort.map(|e| e.as_str()), execution.source])?;
        let id = transaction.last_insert_rowid();
        let owner_pid =
            (execution_mode == crate::registry::AUTOMATED).then_some(i64::from(std::process::id()));
        if transaction.execute(
            "INSERT INTO execution_reservations(agent_id, run_id, owner_pid) VALUES (?1, ?2, ?3)",
            params![agent, id, owner_pid],
        ).is_err() {
            return Err(DbError::AgentHasActiveRun(agent.to_owned()));
        }
        transaction.commit()?;
        let agent_id = agent.to_owned();
        if let Err(error) = self.record_lifecycle_event(
            "dispatch_start",
            Some(task_id),
            Some(id),
            Some(&agent_id),
            None,
        ) {
            let _ = self.update_agent_run_failure(id, None, &error.to_string(), None);
            return Err(error);
        }
        Ok(id)
    }

    pub fn create_project_action_run(
        &self,
        project_id: i64,
        task_id: Option<&str>,
        action: &str,
        agent: &str,
        execution: AgentRunExecution<'_>,
    ) -> Result<i64, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute("INSERT INTO agent_runs (project_id, task_id, agent, execution_mode, execution_class, resolved_model, resolved_reasoning_effort, resolution_source, status, started_at, phase, last_activity) VALUES (?1, ?2, ?3, 'automated', ?4, ?5, ?6, ?7, 'running', CURRENT_TIMESTAMP, ?4, CURRENT_TIMESTAMP)", params![project_id, task_id, agent, action, execution.model, execution.effort.map(|e| e.as_str()), execution.source])?;
        let id = transaction.last_insert_rowid();
        if transaction.execute(
            "INSERT INTO execution_reservations(agent_id, run_id, owner_pid) VALUES (?1, ?2, ?3)",
            params![agent, id, i64::from(std::process::id())],
        ).is_err() {
            return Err(DbError::AgentHasActiveRun(agent.to_owned()));
        }
        transaction.commit()?;
        if let Err(error) = self.record_lifecycle_event(
            "action_start",
            task_id,
            Some(id),
            Some(agent),
            Some(action),
        ) {
            let _ = self.update_agent_run_failure(id, None, &error.to_string(), None);
            return Err(error);
        }
        Ok(id)
    }

    pub fn actionable_revision_review(
        &self,
        task_id: &str,
    ) -> Result<Option<(i64, String)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, output FROM agent_runs WHERE task_id=?1 AND execution_class='review' AND status='completed' AND review_consumed=0 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([task_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (id, Some(output)) = row? else { continue };
            let Ok(review) = serde_json::from_str::<crate::automated::ReviewResult>(&output) else {
                continue;
            };
            return Ok(review
                .verdict
                .eq_ignore_ascii_case("revise")
                .then(|| (id, review.revision_feedback.unwrap_or_default())));
        }
        Ok(None)
    }

    pub fn link_revision_to_review(
        &self,
        revision_run_id: i64,
        review_run_id: i64,
    ) -> Result<bool, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let changed = Self::link_revision_to_review_in_transaction(
            &transaction,
            revision_run_id,
            review_run_id,
        )?;
        if !changed {
            transaction.rollback()?;
            return Ok(false);
        }
        transaction.commit()?;
        Ok(true)
    }

    fn link_revision_to_review_in_transaction(
        conn: &Connection,
        revision_run_id: i64,
        review_run_id: i64,
    ) -> Result<bool, DbError> {
        let changed = conn.execute(
            "UPDATE agent_runs
             SET source_review_run_id = ?1
             WHERE id = ?2
               AND source_review_run_id IS NULL
               AND EXISTS (
                   SELECT 1 FROM agent_runs review
                   WHERE review.id = ?1
                     AND review.task_id = agent_runs.task_id
                     AND review.execution_class = 'review'
                     AND review.status = 'completed'
                     AND review.review_consumed = 0
               )",
            params![review_run_id, revision_run_id],
        )?;
        if changed != 0 {
            let consumed = conn.execute(
                "UPDATE agent_runs SET review_consumed = 1
                 WHERE id = ?1 AND review_consumed = 0",
                [review_run_id],
            )?;
            if consumed == 0 {
                return Ok(false);
            }
        }
        Ok(changed != 0)
    }

    /// Records that a revision has crossed its execution-start boundary.
    ///
    /// The linkage and consumption are one persistent transition so a review
    /// cannot be consumed without the revision run also identifying it.
    pub fn start_revision_execution(
        &self,
        revision_run_id: i64,
        review_run_id: i64,
    ) -> Result<bool, DbError> {
        self.link_revision_to_review(revision_run_id, review_run_id)
    }

    pub fn source_review_run_id(&self, revision_run_id: i64) -> Result<Option<i64>, DbError> {
        Ok(self.conn.query_row(
            "SELECT source_review_run_id FROM agent_runs WHERE id = ?1",
            [revision_run_id],
            |row| row.get(0),
        )?)
    }

    pub fn update_agent_run_status(
        &self,
        run_id: i64,
        status: &str,
        output: Option<&str>,
    ) -> Result<bool, DbError> {
        self.update_agent_run_status_with_usage(run_id, status, output, None)
    }

    pub fn set_agent_run_execution(
        &self,
        run_id: i64,
        class: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
        source: &str,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute("UPDATE agent_runs SET execution_class = ?1, resolved_model = ?2, resolved_reasoning_effort = ?3, resolution_source = ?4 WHERE id = ?5", params![class, model, effort.map(|value| value.as_str()), source, run_id])? != 0)
    }

    pub fn set_agent_run_profile(
        &self,
        run_id: i64,
        profile: Option<&str>,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agent_runs SET resolved_profile = ?1 WHERE id = ?2",
            params![profile, run_id],
        )? != 0)
    }

    pub fn update_agent_run_status_with_usage(
        &self,
        run_id: i64,
        status: &str,
        output: Option<&str>,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<bool, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let terminal = !matches!(status, "running" | "waiting_external");
        let changed = transaction.execute(
            "UPDATE agent_runs
             SET status = ?1, output = ?2,
                 error = CASE WHEN ?1 = 'failed' THEN error ELSE NULL END,
                 finished_at = CASE WHEN ?4 THEN CURRENT_TIMESTAMP ELSE finished_at END,
                 last_activity = CURRENT_TIMESTAMP
             WHERE id = ?3
               AND (?4 = 0 OR status IN ('running', 'waiting_external'))",
            params![status, output, run_id, terminal],
        )?;
        if changed != 0 && !matches!(status, "running" | "waiting_external") {
            transaction.execute(
                "DELETE FROM execution_reservations WHERE run_id = ?1",
                [run_id],
            )?;
        }
        transaction.commit()?;
        if changed != 0 && matches!(status, "completed" | "failed" | "no_changes") {
            self.record_worker_result(run_id, status, output, token_usage)?;
        }
        Ok(changed != 0)
    }

    /// Atomically completes a successful implementation or revision run and
    /// publishes the task for review.
    pub fn complete_agent_run_for_review(
        &self,
        task_id: &str,
        run_id: i64,
        output: &str,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<(), DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let event = Self::complete_agent_run_for_review_in_transaction(
            &transaction,
            task_id,
            run_id,
            output,
            token_usage,
        )?;
        transaction.commit()?;

        if let Some(sink) = &self.lifecycle_sink {
            sink(event);
        }
        Ok(())
    }

    /// Atomically consumes the revision inputs and publishes its successful
    /// run and task for review.
    pub fn complete_revision_run_for_review(
        &self,
        task_id: &str,
        run_id: i64,
        source_review_id: i64,
        contract_id: Option<i64>,
        output: &str,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<bool, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        if !Self::link_revision_to_review_in_transaction(&transaction, run_id, source_review_id)? {
            return Ok(false);
        }
        if let Some(contract_id) = contract_id {
            transaction.execute(
                "UPDATE revision_contracts
                 SET status = 'consumed', consumed_at = CURRENT_TIMESTAMP
                 WHERE id = ?1 AND status = 'actionable'",
                [contract_id],
            )?;
        }
        let event = Self::complete_agent_run_for_review_in_transaction(
            &transaction,
            task_id,
            run_id,
            output,
            token_usage,
        )?;
        transaction.commit()?;

        if let Some(sink) = &self.lifecycle_sink {
            sink(event);
        }
        Ok(true)
    }

    fn complete_agent_run_for_review_in_transaction(
        conn: &Connection,
        task_id: &str,
        run_id: i64,
        output: &str,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<LifecycleEvent, DbError> {
        let changed = conn.execute(
            "UPDATE agent_runs
             SET status = 'completed', output = ?1, error = NULL,
                 finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP
             WHERE id = ?2
               AND task_id = ?3
               AND status IN ('running', 'waiting_external')",
            params![output, run_id, task_id],
        )?;
        if changed == 0 {
            return Err(DbError::InvalidRunStatus(run_id));
        }
        conn.execute(
            "DELETE FROM execution_reservations WHERE run_id = ?1",
            [run_id],
        )?;
        let task_changed = conn.execute(
            "UPDATE tasks SET status = 'review', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            [task_id],
        )?;
        if task_changed == 0 {
            return Err(DbError::TaskNotFound(task_id.to_owned()));
        }
        Self::persist_worker_result_and_event(conn, run_id, "completed", Some(output), token_usage)
    }

    pub fn update_agent_run_failure(
        &self,
        run_id: i64,
        raw_output: Option<&str>,
        error: &str,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<bool, DbError> {
        let transaction = self.conn.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE agent_runs SET status = 'failed', output = ?1, error = ?2, finished_at = CURRENT_TIMESTAMP, last_activity = CURRENT_TIMESTAMP WHERE id = ?3 AND status IN ('running', 'waiting_external')",
            params![raw_output, error, run_id],
        )?;
        if changed != 0 {
            transaction.execute(
                "DELETE FROM execution_reservations WHERE run_id = ?1",
                [run_id],
            )?;
        }
        transaction.commit()?;
        if changed != 0 {
            self.record_worker_result(run_id, "failed", Some(error), token_usage)?;
        }
        Ok(changed != 0)
    }

    fn persist_worker_result_and_event(
        conn: &Connection,
        run_id: i64,
        status: &str,
        output: Option<&str>,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<LifecycleEvent, DbError> {
        let outcome = match status {
            "completed" => "success",
            "no_changes" => "no_changes",
            _ if output.is_some_and(|value| value.to_ascii_lowercase().contains("timed out")) => {
                "timeout"
            }
            _ if output.is_some_and(|value| value.contains("Validation")) => "validation_failure",
            _ => "worker_failure",
        };
        let failure_category = match outcome {
            "success" | "no_changes" => None,
            value => Some(value),
        };
        conn.execute(
            "INSERT OR REPLACE INTO worker_results (run_id, outcome, failure_category, duration_ms, metadata, total_tokens, input_tokens, output_tokens, cached_input_tokens) SELECT ?1, ?2, ?3, (unixepoch(finished_at) - unixepoch(started_at)) * 1000, ?4, ?5, ?6, ?7, ?8 FROM agent_runs WHERE id = ?1",
            params![
                run_id,
                outcome,
                failure_category,
                format!("{{\"run_status\":\"{status}\"}}"),
                token_usage.map(|usage| usage.total_tokens),
                token_usage.and_then(|usage| usage.input_tokens),
                token_usage.and_then(|usage| usage.output_tokens),
                token_usage.and_then(|usage| usage.cached_input_tokens),
            ],
        )?;
        let (task_id, agent_id): (Option<String>, String) = conn.query_row(
            "SELECT task_id, agent FROM agent_runs WHERE id = ?1",
            params![run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let payload = format!("{{\"outcome\":\"{outcome}\"}}");
        conn.execute(
            "INSERT INTO lifecycle_events (kind, task_id, run_id, agent_id, payload)
             VALUES ('worker_result', ?1, ?2, ?3, ?4)",
            params![task_id, run_id, agent_id, payload],
        )?;
        let id = conn.last_insert_rowid();
        let timestamp = conn.query_row(
            "SELECT timestamp FROM lifecycle_events WHERE id = ?1",
            [id],
            |row| row.get(0),
        )?;
        Ok(LifecycleEvent {
            id,
            timestamp,
            kind: "worker_result".to_owned(),
            task_id,
            run_id: Some(run_id),
            agent_id: Some(agent_id),
            payload: Some(payload),
        })
    }

    fn record_worker_result(
        &self,
        run_id: i64,
        status: &str,
        output: Option<&str>,
        token_usage: Option<crate::worker::TokenUsage>,
    ) -> Result<(), DbError> {
        let event =
            Self::persist_worker_result_and_event(&self.conn, run_id, status, output, token_usage)?;
        if let Some(sink) = &self.lifecycle_sink {
            sink(event);
        }
        Ok(())
    }

    pub fn insert_worker_result(&self, result: &WorkerResult) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO worker_results (run_id, outcome, failure_category, duration_ms, metadata, total_tokens, input_tokens, output_tokens, cached_input_tokens) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![result.run_id, result.outcome, result.failure_category, result.duration_ms, result.metadata, result.total_tokens, result.input_tokens, result.output_tokens, result.cached_input_tokens],
        )?;
        Ok(())
    }

    pub fn get_worker_result(&self, run_id: i64) -> Result<Option<WorkerResult>, DbError> {
        Ok(self.conn.query_row(
            "SELECT run_id, outcome, failure_category, duration_ms, metadata, total_tokens, input_tokens, output_tokens, cached_input_tokens FROM worker_results WHERE run_id = ?1",
            params![run_id],
            |row| Ok(WorkerResult { run_id: row.get(0)?, outcome: row.get(1)?, failure_category: row.get(2)?, duration_ms: row.get(3)?, metadata: row.get(4)?, total_tokens: row.get(5)?, input_tokens: row.get(6)?, output_tokens: row.get(7)?, cached_input_tokens: row.get(8)? }),
        ).optional()?)
    }

    pub fn set_agent_run_waiting_external(&self, run_id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE agent_runs SET status = 'waiting_external' WHERE id = ?1 AND status = 'running'",
            params![run_id],
        )? != 0)
    }

    pub fn get_agent_run(&self, run_id: i64) -> Result<Option<AgentRun>, DbError> {
        Ok(self.conn.query_row(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, error, started_at, finished_at, phase, last_activity, execution_class, resolved_model, resolved_reasoning_effort, resolution_source, resolved_profile FROM agent_runs WHERE id = ?1",
            params![run_id], Self::agent_run_from_row).optional()?)
    }

    pub fn complete_manual_run(&self, run_id: i64, output: &str) -> Result<String, DbError> {
        let task_id: String = self.conn.query_row(
            "SELECT task_id FROM agent_runs WHERE id = ?1 AND status = 'waiting_external'",
            params![run_id],
            |row| row.get(0),
        )?;
        self.complete_agent_run_for_review(&task_id, run_id, output, None)?;
        Ok(task_id)
    }

    pub fn fail_run(&self, run_id: i64, reason: &str) -> Result<String, DbError> {
        let task_id: String = self.conn.query_row(
            "SELECT task_id FROM agent_runs WHERE id = ?1 AND status IN ('running', 'waiting_external')",
            params![run_id], |row| row.get(0))?;
        let transaction = self.conn.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE agent_runs SET status = 'failed', output = ?1, finished_at = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')",
            params![reason, run_id])?;
        if changed == 0 {
            return Err(DbError::InvalidRunStatus(run_id));
        }
        transaction.execute(
            "DELETE FROM execution_reservations WHERE run_id = ?1",
            [run_id],
        )?;
        if transaction.execute(
            "UPDATE tasks SET status = 'blocked', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            [&task_id],
        )? == 0
        {
            return Err(DbError::TaskNotFound(task_id));
        }
        let event = Self::persist_worker_result_and_event(
            &transaction,
            run_id,
            "failed",
            Some(reason),
            None,
        )?;
        transaction.commit()?;
        if let Some(sink) = &self.lifecycle_sink {
            sink(event);
        }
        Ok(task_id)
    }

    pub fn list_agent_runs(&self, project_id: i64, limit: usize) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, error, started_at, finished_at, phase, last_activity, execution_class, resolved_model, resolved_reasoning_effort, resolution_source, resolved_profile FROM agent_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT ?2",
        )?;
        Ok(stmt
            .query_map(params![project_id, limit as i64], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_agent_runs_for_task(&self, task_id: &str) -> Result<Vec<AgentRun>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, task_id, agent, execution_mode, status, output, error, started_at, finished_at, phase, last_activity, execution_class, resolved_model, resolved_reasoning_effort, resolution_source, resolved_profile FROM agent_runs WHERE task_id = ?1 ORDER BY started_at DESC, id DESC",
        )?;
        Ok(stmt
            .query_map(params![task_id], Self::agent_run_from_row)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_busy_agents(&self) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT r.agent_id FROM execution_reservations r
             JOIN agent_runs run ON run.id = r.run_id AND run.agent = r.agent_id
             WHERE run.status IN ('running', 'waiting_external') ORDER BY r.agent_id",
        )?;
        Ok(stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn update_agent_run_output(&self, run_id: i64, output: &str) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "UPDATE agent_runs SET output = ?1 WHERE id = ?2",
            params![output, run_id],
        )?;
        Ok(changed != 0)
    }

    pub fn update_agent_run_phase(&self, run_id: i64, phase: &str) -> Result<bool, DbError> {
        let changed = self.conn.execute("UPDATE agent_runs SET phase = ?1, last_activity = CURRENT_TIMESTAMP WHERE id = ?2 AND status IN ('running', 'waiting_external')", params![phase, run_id])? != 0;
        if changed {
            let kind = if phase == "validation started" {
                "validation_started"
            } else if phase == "validation completed" {
                "validation_completed"
            } else {
                "run_phase_changed"
            };
            self.record_lifecycle_event(kind, None, Some(run_id), None, Some(phase))?;
        }
        Ok(changed)
    }

    pub fn record_worker_output(&self, run_id: i64, output: &str) -> Result<i64, DbError> {
        self.touch_agent_run_activity(run_id)?;
        self.record_lifecycle_event("worker_output", None, Some(run_id), None, Some(output))
    }

    pub fn list_worker_output(&self, run_id: i64) -> Result<Vec<LifecycleEvent>, DbError> {
        let mut stmt = self.conn.prepare("SELECT id, timestamp, kind, task_id, run_id, agent_id, payload FROM lifecycle_events WHERE run_id = ?1 AND kind = 'worker_output' ORDER BY id ASC")?;
        Ok(stmt
            .query_map(params![run_id], |row| {
                Ok(LifecycleEvent {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    kind: row.get(2)?,
                    task_id: row.get(3)?,
                    run_id: row.get(4)?,
                    agent_id: row.get(5)?,
                    payload: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn touch_agent_run_activity(&self, run_id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute("UPDATE agent_runs SET last_activity = CURRENT_TIMESTAMP WHERE id = ?1 AND status IN ('running', 'waiting_external')", params![run_id])? != 0)
    }

    pub fn store_worktree_metadata(
        &self,
        agent_run_id: i64,
        task_id: &str,
        branch_name: &str,
        worktree_path: &str,
    ) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO worktree_metadata (agent_run_id, task_id, branch_name, worktree_path) VALUES (?1, ?2, ?3, ?4)",
            params![agent_run_id, task_id, branch_name, worktree_path],
        )?;
        Ok(())
    }

    pub fn get_worktree_metadata(
        &self,
        task_id: &str,
    ) -> Result<Option<(String, String)>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT branch_name, worktree_path FROM worktree_metadata WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
                params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn get_worktree_metadata_for_run(
        &self,
        run_id: i64,
    ) -> Result<Option<(String, String)>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT branch_name, worktree_path FROM worktree_metadata WHERE agent_run_id = ?1",
                params![run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    pub fn validate_task_purge(&self, id: &str, force: bool) -> Result<(), DbError> {
        let task = self
            .get_task(id)?
            .ok_or_else(|| DbError::TaskNotFound(id.to_owned()))?;
        let active: bool = self.conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_runs WHERE task_id = ?1 AND status IN ('running', 'waiting_external'))", [id], |r| r.get(0))?;
        if active {
            return Err(DbError::TaskPurgeActiveRun(id.to_owned()));
        }
        if !force && !task.status.is_terminal() {
            return Err(DbError::TaskPurgeNotTerminal(id.to_owned()));
        }
        let dependents = self.list_task_dependents(id)?;
        if !force && !dependents.is_empty() {
            return Err(DbError::TaskPurgeHasDependents(
                id.to_owned(),
                dependents.join(", "),
            ));
        }
        Ok(())
    }

    pub fn purge_task(&self, id: &str, force: bool) -> Result<Option<String>, DbError> {
        self.validate_task_purge(id, force)?;
        let path = self.get_worktree_metadata(id)?.map(|(_, path)| path);
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            self.conn
                .execute("DELETE FROM task_dependencies WHERE task_id = ?1", [id])?;
            if force {
                self.conn
                    .execute("DELETE FROM task_dependencies WHERE depends_on = ?1", [id])?;
            }
            self.conn.execute(
                "DELETE FROM worker_results WHERE run_id IN (SELECT id FROM agent_runs WHERE task_id = ?1)",
                [id],
            )?;
            self.conn.execute(
                "DELETE FROM run_change_evidence WHERE run_id IN (SELECT id FROM agent_runs WHERE task_id = ?1)",
                [id],
            )?;
            self.conn
                .execute("DELETE FROM lifecycle_events WHERE task_id = ?1 OR run_id IN (SELECT id FROM agent_runs WHERE task_id = ?1)", [id])?;
            self.conn.execute(
                "DELETE FROM worktree_metadata WHERE task_id = ?1 OR agent_run_id IN (SELECT id FROM agent_runs WHERE task_id = ?1)",
                [id],
            )?;
            self.conn
                .execute("DELETE FROM decisions WHERE task_id = ?1", [id])?;
            self.conn
                .execute("DELETE FROM agent_runs WHERE task_id = ?1", [id])?;
            self.conn.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
            self.conn.execute_batch("COMMIT")?;
            Ok(path)
        })();
        if result.is_err() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        result
    }

    pub fn add_task_dependency(&self, task_id: &str, depends_on: &str) -> Result<(), DbError> {
        if task_id == depends_on {
            return Err(DbError::SelfDependency(task_id.to_string()));
        }

        let task_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM tasks WHERE id = ?1",
                params![task_id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !task_exists {
            return Err(DbError::TaskNotFound(task_id.to_string()));
        }

        let depends_on_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM tasks WHERE id = ?1",
                params![depends_on],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !depends_on_exists {
            return Err(DbError::TaskNotFound(depends_on.to_string()));
        }

        let already_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
                params![task_id, depends_on],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if already_exists {
            return Err(DbError::DuplicateDependency(
                task_id.to_string(),
                depends_on.to_string(),
            ));
        }

        // Cycle check: can `depends_on` reach `task_id` via existing dependencies?
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(depends_on.to_string());
        visited.insert(depends_on.to_string());

        while let Some(current) = queue.pop_front() {
            let deps = self.list_task_dependencies(&current)?;
            for dep in deps {
                if dep == task_id {
                    return Err(DbError::DependencyCycle(
                        task_id.to_string(),
                        depends_on.to_string(),
                    ));
                }
                if visited.insert(dep.clone()) {
                    queue.push_back(dep);
                }
            }
        }

        self.conn.execute(
            "INSERT INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
            params![task_id, depends_on],
        )?;
        Ok(())
    }

    pub fn remove_task_dependency(&self, task_id: &str, depends_on: &str) -> Result<bool, DbError> {
        let changed = self.conn.execute(
            "DELETE FROM task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
            params![task_id, depends_on],
        )?;
        Ok(changed != 0)
    }

    pub fn list_task_dependencies(&self, task_id: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT depends_on FROM task_dependencies WHERE task_id = ?1 ORDER BY depends_on",
        )?;
        let rows = stmt.query_map(params![task_id], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_task_dependents(&self, task_id: &str) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id FROM task_dependencies WHERE depends_on = ?1 ORDER BY task_id",
        )?;
        let rows = stmt.query_map(params![task_id], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn list_all_dependencies(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, depends_on FROM task_dependencies ORDER BY task_id, depends_on",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}
