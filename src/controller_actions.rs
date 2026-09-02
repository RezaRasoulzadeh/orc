//! Typed, read-only Controller action intents and kernel legality inspection.
//!
//! Controller code can propose one of the small high-level intents below, but
//! it cannot provide commands, persistence handles or execution arguments.
//! Legality is delegated to [`ProjectOperations`], which owns the canonical
//! queue, lifecycle and evidence projections.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::operations::{OperationalAction, ProjectOperations};

pub use crate::operations::{
    OperationalAction as ControllerActionKind,
    OperationalActionLegality as ControllerActionLegality,
    OperationalActionObservation as ControllerActionObservation,
    OperationalActionRejection as ControllerActionRejection,
};

const MAX_CONTROLLER_ACTION_TASK_ID_BYTES: usize = 256;

/// The only action proposals currently inspectable at the Controller boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerActionIntent {
    Dispatch { task_id: String },
    SemanticReview { task_id: String },
    Revise { task_id: String },
    Accept { task_id: String },
}

#[derive(Debug, Error)]
pub enum ControllerActionError {
    #[error(
        "controller action task ID must be non-empty and at most {MAX_CONTROLLER_ACTION_TASK_ID_BYTES} bytes"
    )]
    InvalidTaskId,
    #[error("controller action legality read failed: {0}")]
    Read(#[source] anyhow::Error),
}

impl ControllerActionIntent {
    pub fn action_kind(&self) -> ControllerActionKind {
        match self {
            Self::Dispatch { .. } => OperationalAction::Dispatch,
            Self::SemanticReview { .. } => OperationalAction::SemanticReview,
            Self::Revise { .. } => OperationalAction::Revise,
            Self::Accept { .. } => OperationalAction::Accept,
        }
    }

    pub fn task_id(&self) -> &str {
        match self {
            Self::Dispatch { task_id }
            | Self::SemanticReview { task_id }
            | Self::Revise { task_id }
            | Self::Accept { task_id } => task_id,
        }
    }

    pub fn validate(&self) -> std::result::Result<(), ControllerActionError> {
        let task_id = self.task_id();
        if task_id.trim().is_empty() || task_id.len() > MAX_CONTROLLER_ACTION_TASK_ID_BYTES {
            return Err(ControllerActionError::InvalidTaskId);
        }
        Ok(())
    }

    /// Ask the deterministic kernel whether this intent is legal. This method
    /// only reads canonical state and never executes or persists the intent.
    pub fn inspect(
        &self,
        operations: &ProjectOperations<'_>,
    ) -> std::result::Result<ControllerActionLegality, ControllerActionError> {
        self.validate()?;
        operations
            .inspect_action(self.task_id(), self.action_kind())
            .map_err(ControllerActionError::Read)
    }
}

/// Convenience function for callers that prefer a free inspection boundary.
pub fn inspect_action(
    intent: &ControllerActionIntent,
    operations: &ProjectOperations<'_>,
) -> std::result::Result<ControllerActionLegality, ControllerActionError> {
    intent.inspect(operations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    use crate::automated::ReviewResult;
    use crate::operations::ProjectOperations;
    use crate::registry::{self, AgentAction, AgentDefinition, ReasoningEffort};
    use crate::storage::{AgentRunExecution, Database};
    use crate::task::{CreateTaskInput, TaskPriority, TaskStatus};
    use crate::validation::{ValidationCategory, ValidationReport, ValidationStepResult};
    use tempfile::TempDir;

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
        run_git(directory.path(), &["init", "."]);
        run_git(
            directory.path(),
            &["config", "user.email", "controller-actions@example.com"],
        );
        run_git(
            directory.path(),
            &["config", "user.name", "Controller Actions Test"],
        );
        std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
        run_git(directory.path(), &["add", "README.md"]);
        run_git(directory.path(), &["commit", "-m", "base"]);
        directory
    }

    fn agent() -> AgentDefinition {
        AgentDefinition {
            id: "agent-a".into(),
            backend: "codex".into(),
            execution_mode: registry::AUTOMATED.into(),
            display_name: "Test agent".into(),
            enabled: true,
            priority: 1,
            capabilities: vec!["code".into(), "command_execution".into(), "review".into()],
            status: registry::AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: None,
            model: Some("test-model".into()),
            reasoning_effort: Some(ReasoningEffort::Low),
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![AgentAction::Code, AgentAction::Review],
        }
    }

    fn setup() -> (TempDir, Database, i64, String) {
        let repo = repository();
        let db = Database::init(repo.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("controller-actions").unwrap();
        db.insert_agent(&agent()).unwrap();
        let task = db
            .create_task(
                project,
                &CreateTaskInput {
                    title: "Controller action task".into(),
                    objective: "Inspect action legality".into(),
                    role: "developer".into(),
                    priority: TaskPriority::Normal,
                    required_capabilities: Vec::new(),
                    scope_mode: None,
                    context_files: Vec::new(),
                    expected_changes: Vec::new(),
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        (repo, db, project, task)
    }

    fn create_run(db: &Database, project: i64, task: &str, class: &str) -> i64 {
        db.create_agent_run_with_execution(
            project,
            task,
            "agent-a",
            registry::AUTOMATED,
            AgentRunExecution {
                class,
                model: Some("test-model"),
                effort: Some(ReasoningEffort::Low),
                source: "controller-actions-test",
            },
        )
        .unwrap()
    }

    fn review_output(verdict: &str) -> String {
        serde_json::to_string(&ReviewResult {
            verdict: verdict.into(),
            criterion_results: Vec::new(),
            findings: Vec::new(),
            blocking_findings: Vec::new(),
            non_blocking_findings: Vec::new(),
            severity: None,
            revision_feedback: Some("test feedback".into()),
            blockers: Vec::new(),
        })
        .unwrap()
    }

    fn passing_validation() -> ValidationReport {
        ValidationReport {
            steps: vec![ValidationStepResult {
                command: "cargo test".into(),
                category: ValidationCategory::Success,
                passed: true,
                stdout: String::new(),
                stderr: String::new(),
                exit_status: Some(0),
                diagnostics: None,
                failure_classification: None,
                fallback_command: None,
            }],
        }
    }

    fn persist_validation(db: &Database, task: &str, run: i64, report: &ValidationReport) {
        db.record_lifecycle_event(
            "validation_result",
            Some(task),
            Some(run),
            Some("agent-a"),
            Some(&serde_json::to_string(report).unwrap()),
        )
        .unwrap();
    }

    fn rejected(result: ControllerActionLegality) -> ControllerActionRejection {
        match result {
            ControllerActionLegality::Rejected { reason, .. } => reason,
            ControllerActionLegality::Allowed { .. } => panic!("expected rejected action"),
        }
    }

    #[test]
    fn action_intents_are_serializable_and_typed() {
        let intents = [
            ControllerActionIntent::Dispatch {
                task_id: "T-0001".into(),
            },
            ControllerActionIntent::SemanticReview {
                task_id: "T-0001".into(),
            },
            ControllerActionIntent::Revise {
                task_id: "T-0001".into(),
            },
            ControllerActionIntent::Accept {
                task_id: "T-0001".into(),
            },
        ];
        for intent in intents {
            let encoded = serde_json::to_string(&intent).unwrap();
            let decoded: ControllerActionIntent = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, intent);
            assert_eq!(decoded.task_id(), "T-0001");
            assert!(decoded.validate().is_ok());
        }
    }

    #[test]
    fn action_intent_rejects_unbounded_or_blank_task_ids() {
        let blank = ControllerActionIntent::Dispatch {
            task_id: "   ".into(),
        };
        assert!(matches!(
            blank.validate(),
            Err(ControllerActionError::InvalidTaskId)
        ));
        let oversized = ControllerActionIntent::Accept {
            task_id: "x".repeat(MAX_CONTROLLER_ACTION_TASK_ID_BYTES + 1),
        };
        assert!(matches!(
            oversized.validate(),
            Err(ControllerActionError::InvalidTaskId)
        ));
        assert!(
            serde_json::from_str::<ControllerActionIntent>(
                r#"{"kind":"dispatch","task_id":"T-0001","command":"rm -rf"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn dispatch_legality_uses_canonical_queue_and_dependencies() {
        let (repo, db, _project, task) = setup();
        db.update_task_status(&task, TaskStatus::Ready).unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        let dispatch = ControllerActionIntent::Dispatch {
            task_id: task.clone(),
        }
        .inspect(&operations)
        .unwrap();
        assert!(matches!(dispatch, ControllerActionLegality::Allowed { .. }));
        let encoded = serde_json::to_string(&dispatch).unwrap();
        let decoded: ControllerActionLegality = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, dispatch);

        let (repo, db, project, dependent) = setup();
        let dependency = db
            .create_task(
                project,
                &CreateTaskInput {
                    title: "Dependency".into(),
                    objective: "Dependency".into(),
                    role: "developer".into(),
                    priority: TaskPriority::Normal,
                    required_capabilities: Vec::new(),
                    scope_mode: None,
                    context_files: Vec::new(),
                    expected_changes: Vec::new(),
                    dependencies: Vec::new(),
                },
            )
            .unwrap();
        db.add_task_dependency(&dependent, &dependency).unwrap();
        db.update_task_status(&dependent, TaskStatus::Ready)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        let blocked = ControllerActionIntent::Dispatch { task_id: dependent }
            .inspect(&operations)
            .unwrap();
        let encoded = serde_json::to_string(&blocked).unwrap();
        let decoded: ControllerActionLegality = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, blocked);
        assert!(matches!(
            rejected(blocked),
            ControllerActionRejection::DependenciesIncomplete { .. }
        ));

        let (repo, db, _project, no_agent) = setup();
        db.set_task_required_capabilities(&no_agent, &["unavailable-capability".into()])
            .unwrap();
        db.update_task_status(&no_agent, TaskStatus::Ready).unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Dispatch { task_id: no_agent }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::NoEligibleAgent
        ));
    }

    #[test]
    fn semantic_review_legality_requires_current_passing_validation() {
        let (repo, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::Review).unwrap();
        let run = create_run(&db, project, &task, "coder");
        db.update_agent_run_status(run, "completed", Some("implementation"))
            .unwrap();
        persist_validation(&db, &task, run, &passing_validation());
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::SemanticReview {
                task_id: task.clone()
            }
            .inspect(&operations)
            .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));

        let (repo, db, project, missing) = setup();
        db.update_task_status(&missing, TaskStatus::Review).unwrap();
        let run = create_run(&db, project, &missing, "coder");
        db.store_worktree_metadata(run, &missing, "branch", ".")
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::SemanticReview { task_id: missing }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ValidationMissing
        ));

        let (repo, db, project, stale) = setup();
        db.update_task_status(&stale, TaskStatus::Review).unwrap();
        let run = create_run(&db, project, &stale, "coder");
        db.update_agent_run_status(run, "completed", Some("implementation"))
            .unwrap();
        db.store_worktree_metadata(run, &stale, "branch", ".")
            .unwrap();
        persist_validation(&db, &stale, run, &passing_validation());
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::SemanticReview { task_id: stale }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ValidationStale
        ));

        let (repo, db, project, active) = setup();
        db.update_task_status(&active, TaskStatus::Active).unwrap();
        let run = create_run(&db, project, &active, "coder");
        db.update_agent_run_status(run, "completed", Some("implementation"))
            .unwrap();
        persist_validation(&db, &active, run, &passing_validation());
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::SemanticReview { task_id: active }
                .inspect(&operations)
                .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));

        let (repo, db, _project, terminal) = setup();
        db.update_task_status(&terminal, TaskStatus::Done).unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::SemanticReview { task_id: terminal }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::WrongLifecycle { .. }
        ));
    }

    #[test]
    fn revise_legality_requires_actionable_review_and_no_condition() {
        let (repo, db, project, task) = setup();
        db.update_task_status(&task, TaskStatus::RevisionRequired)
            .unwrap();
        let review = create_run(&db, project, &task, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("REVISE")))
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::Revise {
                task_id: task.clone()
            }
            .inspect(&operations)
            .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));
        db.set_task_execution_condition(&task, "operator_gate", "needs decision")
            .unwrap();
        assert!(matches!(
            rejected(
                ControllerActionIntent::Revise { task_id: task }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ExecutionConditionPresent
        ));

        let (repo, db, _project, missing) = setup();
        db.update_task_status(&missing, TaskStatus::RevisionRequired)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Revise { task_id: missing }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::RevisionEvidenceMissing
        ));
    }

    #[test]
    fn accept_legality_requires_current_pass_review_evidence() {
        let (repo, db, project, task) = setup();
        let implementation = create_run(&db, project, &task, "coder");
        db.store_worktree_metadata(implementation, &task, "branch", ".")
            .unwrap();
        std::fs::write(repo.path().join("accepted.txt"), "change\n").unwrap();
        let changes = crate::git::inspect_worktree(repo.path(), repo.path()).unwrap();
        db.update_agent_run_status(implementation, "completed", Some("implementation"))
            .unwrap();
        let review = create_run(&db, project, &task, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.store_change_evidence(review, &changes).unwrap();
        db.update_task_status(&task, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            ControllerActionIntent::Accept {
                task_id: task.clone()
            }
            .inspect(&operations)
            .unwrap(),
            ControllerActionLegality::Allowed { .. }
        ));

        let (repo, db, _project, missing) = setup();
        db.update_task_status(&missing, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Accept { task_id: missing }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ReviewMissing
        ));

        let (repo, db, project, stale) = setup();
        let review = create_run(&db, project, &stale, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.update_task_status(&stale, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        assert!(matches!(
            rejected(
                ControllerActionIntent::Accept { task_id: stale }
                    .inspect(&operations)
                    .unwrap()
            ),
            ControllerActionRejection::ReviewStale
        ));
    }

    #[test]
    fn action_inspection_is_side_effect_free() {
        let (repo, db, project, task) = setup();
        let implementation = create_run(&db, project, &task, "coder");
        db.store_worktree_metadata(implementation, &task, "branch", ".")
            .unwrap();
        std::fs::write(repo.path().join("unchanged.txt"), "change\n").unwrap();
        let changes = crate::git::inspect_worktree(repo.path(), repo.path()).unwrap();
        db.update_agent_run_status(implementation, "completed", Some("implementation"))
            .unwrap();
        let review = create_run(&db, project, &task, "review");
        db.update_agent_run_status(review, "completed", Some(&review_output("PASS")))
            .unwrap();
        db.store_change_evidence(review, &changes).unwrap();
        db.update_task_status(&task, TaskStatus::AcceptanceReady)
            .unwrap();
        let operations = ProjectOperations::new(&db, repo.path());
        let before_detail = operations.task_detail(&task).unwrap();
        let before_task = db.get_task(&task).unwrap();
        let before_runs =
            serde_json::to_value(db.list_agent_runs_for_task(&task).unwrap()).unwrap();
        let before_worktree = db.get_worktree_metadata(&task).unwrap();
        let before_evidence = db.get_change_evidence(review).unwrap();

        let _ = ControllerActionIntent::Accept {
            task_id: task.clone(),
        }
        .inspect(&operations)
        .unwrap();

        assert_eq!(operations.task_detail(&task).unwrap(), before_detail);
        assert_eq!(db.get_task(&task).unwrap(), before_task);
        assert_eq!(
            serde_json::to_value(db.list_agent_runs_for_task(&task).unwrap()).unwrap(),
            before_runs
        );
        assert_eq!(db.get_worktree_metadata(&task).unwrap(), before_worktree);
        assert_eq!(db.get_change_evidence(review).unwrap(), before_evidence);
    }
}
