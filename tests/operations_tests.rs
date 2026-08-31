use std::path::Path;
use std::process::Command;

use orc::automated::{ReviewBlocker, ReviewResult, revision_worktree_fingerprint};
use orc::operations::{BlockerState, ProjectOperations, ValidationState};
use orc::registry::{
    self, AgentAction, AgentDefinition, EconomyTier, EscalationLineage, EscalationRequest,
    EscalationTrigger, ReasoningEffort, ResolutionRecord,
};
use orc::storage::{AgentRunExecution, Database};
use orc::task::{TaskPriority, TaskStatus};
use orc::validation::{
    ValidationCategory, ValidationFailureClassification, ValidationReport, ValidationStepResult,
};
use orc::worker::TokenUsage;
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
        &["config", "user.email", "operations@example.com"],
    );
    run_git(
        directory.path(),
        &["config", "user.name", "Operations Test"],
    );
    std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
    run_git(directory.path(), &["add", "README.md"]);
    run_git(directory.path(), &["commit", "-m", "base"]);
    directory
}

fn agent(model: &str) -> AgentDefinition {
    AgentDefinition {
        id: "agent-a".into(),
        backend: "codex".into(),
        execution_mode: registry::AUTOMATED.into(),
        display_name: "Agent A".into(),
        enabled: true,
        priority: 10,
        capabilities: vec!["code".into(), "command_execution".into(), "review".into()],
        status: registry::AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: Some("/profiles/agent-a".into()),
        model: Some(model.into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        config_metadata: None,
        quota_remaining_percent: Some(80),
        quota_reset_at: None,
        quota_checked_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        ),
        quota_source: Some("test".into()),
        quota_limits: None,
        actions: vec![AgentAction::Code, AgentAction::Review],
    }
}

fn setup() -> (TempDir, Database, i64, String) {
    let repo = repository();
    let db = Database::init(repo.path().join(".orc/orc.db")).unwrap();
    let project = db.create_project("operations").unwrap();
    db.insert_agent(&agent("model-current")).unwrap();
    let task = db
        .insert_task(
            project,
            "Operational task",
            "Persist and explain operational truth",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    (repo, db, project, task)
}

fn create_run(db: &Database, project: i64, task: &str, class: &str, model: Option<&str>) -> i64 {
    db.create_agent_run_with_execution(
        project,
        task,
        "agent-a",
        registry::AUTOMATED,
        AgentRunExecution {
            class,
            model,
            effort: Some(ReasoningEffort::Low),
            source: "persisted-test",
        },
    )
    .unwrap()
}

fn resolution(model: &str, tier: EconomyTier, lineage: serde_json::Value) -> ResolutionRecord {
    ResolutionRecord {
        selected_agent: "agent-a".into(),
        selected_model: Some(model.into()),
        effort: Some(ReasoningEffort::Low),
        tier,
        source: "authoritative-resolver".into(),
        escalation_reason: None,
        input_lineage: lineage.to_string(),
        escalation: None,
    }
}

fn passing_report(command: &str) -> ValidationReport {
    ValidationReport {
        steps: vec![ValidationStepResult {
            command: command.into(),
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

fn persist_validation(
    db: &Database,
    task: &str,
    run: i64,
    report: &ValidationReport,
    fingerprint: &str,
) {
    db.record_lifecycle_event(
        "validation_result",
        Some(task),
        Some(run),
        Some("agent-a"),
        Some(&serde_json::to_string(report).unwrap()),
    )
    .unwrap();
    db.record_lifecycle_event(
        "validation_selection",
        Some(task),
        Some(run),
        Some("agent-a"),
        Some(
            &serde_json::json!({
                "selected_commands": report.steps.iter().map(|step| &step.command).collect::<Vec<_>>(),
                "worktree_fingerprint": fingerprint,
            })
            .to_string(),
        ),
    )
    .unwrap();
}

#[test]
fn lifecycle_and_current_latest_run_semantics_survive_restart() {
    let (repo, db, project, task) = setup();
    let historical = create_run(&db, project, &task, "coder", Some("model-historical"));
    let persisted = resolution(
        "model-historical",
        EconomyTier::Default,
        serde_json::json!({
            "action": "code",
            "selection_reason": "cheapest_economy_tier",
            "selection_explanation": "historical selection",
            "quota": {
                "remaining_percent": 80,
                "source": "test",
                "freshness": "fresh",
                "reserve_percent": 5
            }
        }),
    );
    let invocation = db
        .start_provider_invocation_with_resolution(historical, "implementation", 1, &persisted)
        .unwrap();
    db.finish_provider_invocation(invocation, "completed", None)
        .unwrap();
    db.update_agent_run_status(historical, "completed", Some("old"))
        .unwrap();
    db.set_agent_model("agent-a", "model-current").unwrap();

    let current = create_run(&db, project, &task, "coder", Some("model-current"));
    db.update_task_status(&task, TaskStatus::Active).unwrap();
    let before = ProjectOperations::new(&db, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(before.lifecycle, TaskStatus::Active);
    assert_eq!(before.current_run.as_ref().unwrap().id, current);
    assert_eq!(before.latest_run.as_ref().unwrap().id, current);
    assert_eq!(
        before.latest_resolution.as_ref().unwrap().model.as_deref(),
        Some("model-historical")
    );
    assert_eq!(
        before
            .latest_resolution
            .as_ref()
            .unwrap()
            .selection_reason
            .as_deref(),
        Some("cheapest_economy_tier")
    );

    let path = repo.path().join(".orc/orc.db");
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let after = ProjectOperations::new(&reopened, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn validation_freshness_and_infrastructure_failure_are_explicit() {
    let (repo, db, project, task) = setup();
    let run = create_run(&db, project, &task, "coder", Some("model-a"));
    db.store_worktree_metadata(run, &task, "operations", ".")
        .unwrap();
    std::fs::write(repo.path().join("change.txt"), "one\n").unwrap();
    let changes = orc::git::inspect_worktree(repo.path(), repo.path()).unwrap();
    let fingerprint = revision_worktree_fingerprint(&changes);
    persist_validation(&db, &task, run, &passing_report("cargo test"), &fingerprint);
    db.store_change_evidence(run, &changes).unwrap();
    db.update_agent_run_status(run, "completed", Some("done"))
        .unwrap();
    db.update_task_status(&task, TaskStatus::Review).unwrap();

    let fresh = ProjectOperations::new(&db, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(fresh.validation.state, ValidationState::Passing);
    assert_eq!(fresh.validation.is_current, Some(true));
    assert_eq!(fresh.validation.latest_passing_run_id, Some(run));
    assert!(fresh.validation.latest_passing_timestamp.is_some());
    assert!(fresh.review.ready_for_review);

    std::fs::write(repo.path().join("change.txt"), "two\n").unwrap();
    let stale = ProjectOperations::new(&db, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(stale.validation.state, ValidationState::Stale);
    assert_eq!(
        stale.validation.recorded_state,
        Some(ValidationState::Passing)
    );
    assert_eq!(stale.validation.is_current, Some(false));

    let current = orc::git::inspect_worktree(repo.path(), repo.path()).unwrap();
    let failing = ValidationReport {
        steps: vec![ValidationStepResult {
            command: "cargo test".into(),
            category: ValidationCategory::Test,
            passed: false,
            stdout: String::new(),
            stderr: "assertion failed".into(),
            exit_status: Some(101),
            diagnostics: Some("tests::semantic".into()),
            failure_classification: Some(ValidationFailureClassification::Implementation),
            fallback_command: None,
        }],
    };
    let current_fingerprint = revision_worktree_fingerprint(&current);
    persist_validation(&db, &task, run, &failing, &current_fingerprint);
    let ordinary_failure = ProjectOperations::new(&db, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(ordinary_failure.validation.state, ValidationState::Failing);
    assert_eq!(
        ordinary_failure.validation.failure_classification,
        Some(ValidationFailureClassification::Implementation)
    );

    let infrastructure = ValidationReport::infrastructure_failure(
        "cargo test",
        "provider-independent test infrastructure unavailable".into(),
    );
    persist_validation(&db, &task, run, &infrastructure, &current_fingerprint);
    let failed = ProjectOperations::new(&db, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(
        failed.validation.state,
        ValidationState::InfrastructureFailure
    );
    assert_eq!(
        failed.validation.failure_classification,
        Some(ValidationFailureClassification::Infrastructure)
    );
}

#[test]
fn review_and_blocker_views_use_current_persisted_ledger() {
    let (repo, db, project, task) = setup();
    let implementation = create_run(&db, project, &task, "coder", Some("model-a"));
    db.store_worktree_metadata(implementation, &task, "operations", ".")
        .unwrap();
    std::fs::write(repo.path().join("reviewed.txt"), "reviewed\n").unwrap();
    let changes = orc::git::inspect_worktree(repo.path(), repo.path()).unwrap();
    db.update_agent_run_status(implementation, "completed", Some("implementation"))
        .unwrap();

    let review = create_run(&db, project, &task, "review", Some("review-model"));
    let output = serde_json::to_string(&ReviewResult {
        verdict: "REVISE".into(),
        findings: vec!["fix semantics".into()],
        blocking_findings: vec!["fix semantics".into()],
        non_blocking_findings: Vec::new(),
        severity: Some("high".into()),
        revision_feedback: Some("fix semantics".into()),
        blockers: Vec::new(),
    })
    .unwrap();
    db.update_agent_run_status(review, "completed", Some(&output))
        .unwrap();
    db.store_change_evidence(review, &changes).unwrap();
    db.store_review_blockers(
        &task,
        review,
        &[
            ReviewBlocker {
                id: "BLK-2".into(),
                prior_blocker_id: None,
                blocker_key: "regression".into(),
                requirement_ref: "criterion-2".into(),
                evidence: "regressed".into(),
                severity: "high".into(),
                acceptance_condition: "restored".into(),
                status: "regression".into(),
                finding: "regressed behavior".into(),
            },
            ReviewBlocker {
                id: "BLK-1".into(),
                prior_blocker_id: None,
                blocker_key: "semantic".into(),
                requirement_ref: "criterion-1".into(),
                evidence: "missing".into(),
                severity: "high".into(),
                acceptance_condition: "implemented".into(),
                status: "unresolved".into(),
                finding: "missing behavior".into(),
            },
            ReviewBlocker {
                id: "BLK-0".into(),
                prior_blocker_id: None,
                blocker_key: "old".into(),
                requirement_ref: "criterion-0".into(),
                evidence: "fixed".into(),
                severity: "low".into(),
                acceptance_condition: "preserved".into(),
                status: "resolved".into(),
                finding: "old issue".into(),
            },
        ],
    )
    .unwrap();
    db.update_task_status(&task, TaskStatus::RevisionRequired)
        .unwrap();

    let detail = ProjectOperations::new(&db, repo.path())
        .task_detail(&task)
        .unwrap()
        .unwrap();
    assert_eq!(detail.summary.review.verdict.as_deref(), Some("REVISE"));
    assert_eq!(detail.summary.review.applies_to_current_change, Some(true));
    assert_eq!(detail.summary.review.actionable_blockers, 2);
    assert_eq!(detail.summary.review.regressed_blockers, 1);
    assert_eq!(detail.summary.review.resolved_blockers, 1);
    assert!(detail.blockers[0].actionable);
    assert!(detail.blockers[1].actionable);
    assert_eq!(detail.blockers[2].state, BlockerState::Resolved);

    let newer = create_run(&db, project, &task, "coder", Some("model-a"));
    db.update_agent_run_status(newer, "completed", Some("revision"))
        .unwrap();
    let stale = ProjectOperations::new(&db, repo.path())
        .task_summary(&task)
        .unwrap()
        .unwrap();
    assert_eq!(
        stale.review.applies_to_current_change,
        Some(false),
        "a newer implementation deterministically stales the prior review"
    );
}

#[test]
fn economy_metrics_aggregate_invocations_without_double_counting_cached_input() {
    let (repo, db, project, task) = setup();
    let run = create_run(&db, project, &task, "coder", Some("cheap-model"));
    let first = resolution(
        "cheap-model",
        EconomyTier::Default,
        serde_json::json!({
            "action": "code",
            "operator_model": "cheap-model",
            "selection_reason": "operator_override",
            "quota": {
                "remaining_percent": 70,
                "checked_at": "100",
                "source": "provider",
                "freshness": "fresh",
                "reserve_percent": 10,
                "refresh_supported": true
            }
        }),
    );
    let first_invocation = db
        .start_provider_invocation_with_resolution(run, "implementation", 1, &first)
        .unwrap();
    db.finish_provider_invocation(
        first_invocation,
        "completed",
        Some(TokenUsage {
            total_tokens: 100,
            input_tokens: Some(80),
            output_tokens: Some(20),
            cached_input_tokens: Some(30),
        }),
    )
    .unwrap();
    let request = db
        .persist_escalation_request(
            &task,
            &EscalationRequest {
                reason: "semantic revision did not converge".into(),
                lineage: EscalationLineage {
                    request_id: None,
                    trigger: EscalationTrigger::SemanticRevisionNonConvergence,
                    previous_provider_invocation_id: first_invocation,
                    previous_tier: EconomyTier::Default,
                    previous_model: Some("cheap-model".into()),
                    previous_effort: Some(ReasoningEffort::Low),
                    previous_attempt: 1,
                    requested_minimum_tier: EconomyTier::Escalation,
                    policy_attempt: 1,
                },
            },
        )
        .unwrap();
    let mut escalated = resolution(
        "stronger-model",
        EconomyTier::Escalation,
        serde_json::json!({
            "action": "code",
            "selection_reason": "cheapest_economy_tier",
        }),
    );
    escalated.source = "policy_escalation".into();
    escalated.escalation_reason = Some(request.request.reason.clone());
    escalated.escalation = Some(request.request.lineage.clone());
    let second_invocation = db
        .start_provider_invocation_with_resolution(run, "validation_repair", 1, &escalated)
        .unwrap();
    db.finish_provider_invocation(
        second_invocation,
        "completed",
        Some(TokenUsage {
            total_tokens: 60,
            input_tokens: Some(40),
            output_tokens: Some(20),
            cached_input_tokens: Some(10),
        }),
    )
    .unwrap();
    db.update_agent_run_status_with_usage(
        run,
        "completed",
        Some("done"),
        Some(TokenUsage {
            total_tokens: 60,
            input_tokens: Some(40),
            output_tokens: Some(20),
            cached_input_tokens: Some(10),
        }),
    )
    .unwrap();
    db.update_task_status(&task, TaskStatus::Done).unwrap();

    let unaccepted = db
        .insert_task(
            project,
            "Unaccepted",
            "Passing is not acceptance",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let unaccepted_run = create_run(&db, project, &unaccepted, "coder", Some("cheap-model"));
    let unaccepted_resolution = resolution(
        "cheap-model",
        EconomyTier::Default,
        serde_json::json!({"action": "code"}),
    );
    let unaccepted_invocation = db
        .start_provider_invocation_with_resolution(
            unaccepted_run,
            "implementation",
            1,
            &unaccepted_resolution,
        )
        .unwrap();
    db.finish_provider_invocation(
        unaccepted_invocation,
        "completed",
        Some(TokenUsage {
            total_tokens: 25,
            input_tokens: Some(20),
            output_tokens: Some(5),
            cached_input_tokens: Some(5),
        }),
    )
    .unwrap();
    db.update_agent_run_status(unaccepted_run, "completed", Some("completed"))
        .unwrap();
    db.update_task_status(&unaccepted, TaskStatus::AcceptanceReady)
        .unwrap();

    let operations = ProjectOperations::new(&db, repo.path());
    let detail = operations.task_detail(&task).unwrap().unwrap();
    assert_eq!(detail.resolutions.len(), 2);
    assert!(detail.resolutions[0].operator_override);
    assert!(detail.resolutions[0].escalation_reason.is_none());
    let quota = detail.resolutions[0].quota.as_ref().unwrap();
    assert_eq!(quota.remaining_percent, Some(70));
    assert_eq!(quota.source.as_deref(), Some("provider"));
    assert_eq!(quota.freshness.as_deref(), Some("fresh"));
    assert_eq!(detail.escalations.len(), 1);
    assert_eq!(detail.escalations[0].state, "consumed");
    assert_eq!(
        detail.escalations[0].resulting_invocation_id,
        Some(second_invocation)
    );

    let economy = operations.economy_summary().unwrap();
    assert_eq!(economy.invocation_count, 3);
    assert_eq!(economy.invocations_by_tier[&EconomyTier::Default], 2);
    assert_eq!(economy.invocations_by_tier[&EconomyTier::Escalation], 1);
    assert_eq!(economy.token_usage.total_tokens, Some(185));
    assert_eq!(economy.token_usage.input_tokens, Some(140));
    assert_eq!(economy.token_usage.cached_input_tokens, Some(45));
    assert_eq!(economy.token_usage.uncached_input_tokens, Some(95));
    assert_eq!(economy.accepted_tasks, 1);
    assert_eq!(economy.accepted_token_usage.total_tokens, Some(160));
    assert_eq!(economy.tokens_per_accepted_task, Some(160.0));
}

#[test]
fn legacy_missing_resolution_and_project_ordering_are_explicit() {
    let (repo, db, project, first) = setup();
    let second = db
        .insert_task(
            project,
            "Second",
            "Legacy task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let legacy = create_run(&db, project, &second, "coder", Some("legacy-run-model"));
    db.update_agent_run_status(legacy, "completed", Some("legacy"))
        .unwrap();

    let database_path = repo.path().join(".orc/orc.db");
    drop(db);
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO provider_invocations(parent_run_id, purpose, lineage, attempt, outcome, selected_agent, selected_model, tier) VALUES (?1, 'legacy', 'legacy', 1, 'completed', 'agent-a', 'legacy-provider-model', 'unknown')",
            [legacy],
        )
        .unwrap();
    drop(connection);
    let db = Database::open(&database_path).unwrap();

    let operations = ProjectOperations::new(&db, repo.path());
    let summaries = operations.task_summaries().unwrap();
    assert_eq!(
        summaries
            .iter()
            .map(|summary| summary.task_id.as_str())
            .collect::<Vec<_>>(),
        vec![first.as_str(), second.as_str()]
    );
    let legacy = operations.task_summary(&second).unwrap().unwrap();
    let legacy_resolution = legacy.latest_resolution.as_ref().unwrap();
    assert!(legacy_resolution.legacy_missing_resolution);
    assert!(legacy_resolution.source.is_none());
    assert_eq!(legacy_resolution.tier, EconomyTier::Unknown);
    assert_eq!(
        legacy_resolution.model.as_deref(),
        Some("legacy-provider-model")
    );
    assert_eq!(
        legacy
            .latest_run
            .as_ref()
            .unwrap()
            .persisted_model
            .as_deref(),
        Some("legacy-run-model")
    );
}
