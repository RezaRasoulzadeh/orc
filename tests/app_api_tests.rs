use orc::app::OrcApp;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides, ReviewResult};
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlannedTask};
use orc::registry::{AUTOMATED, AVAILABLE, AgentAction, AgentDefinition, ReasoningEffort};
use orc::storage::{AgentRunExecution, Database};
use orc::task::TaskPriority;
use rusqlite::Connection;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::TryRecvError;
use tempfile::tempdir;

fn app_with_task(name: &str) -> (tempfile::TempDir, OrcApp, String) {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project(name).unwrap();
    if name == "two" {
        db.insert_task(
            project,
            "other",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    }
    let task = db
        .insert_task(
            project,
            name,
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&task, orc::task::TaskStatus::Active)
        .unwrap();
    db.create_agent_run(project, &task, "agent").unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    (directory, app, task)
}

fn persist_review(
    db: &Database,
    project: i64,
    task: &str,
    result: &ReviewResult,
    validation: &str,
) -> i64 {
    let run = db
        .create_agent_run_with_execution(
            project,
            task,
            "reviewer",
            AUTOMATED,
            AgentRunExecution {
                class: "review",
                model: Some("test-model"),
                effort: Some(ReasoningEffort::High),
                source: "test",
            },
        )
        .unwrap();
    db.update_agent_run_status(
        run,
        "completed",
        Some(&serde_json::to_string(result).unwrap()),
    )
    .unwrap();
    db.record_lifecycle_event(
        "validation_result",
        Some(task),
        Some(run),
        Some("reviewer"),
        Some(validation),
    )
    .unwrap();
    run
}

fn review_result(label: &str, verdict: &str) -> ReviewResult {
    ReviewResult {
        verdict: verdict.into(),
        severity: Some(format!("severity-{label}")),
        findings: vec![format!("finding-{label}")],
        blocking_findings: vec![format!("blocking-{label}")],
        non_blocking_findings: vec![format!("non-blocking-{label}")],
        revision_feedback: Some(format!("feedback-{label}")),
        blockers: Vec::new(),
    }
}

struct CountingReviewBackend(AtomicUsize);

impl CountingReviewBackend {
    fn calls(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl ActionBackend for CountingReviewBackend {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<ReasoningEffort>,
    ) -> anyhow::Result<ActionExecution> {
        assert_eq!(action, AgentAction::Review);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ActionExecution {
            output: serde_json::to_string(&ReviewResult {
                verdict: "PASS".into(),
                findings: Vec::new(),
                blocking_findings: Vec::new(),
                non_blocking_findings: Vec::new(),
                severity: None,
                revision_feedback: None,
                blockers: Vec::new(),
            })?,
            token_usage: None,
        })
    }
}

fn reviewer() -> AgentDefinition {
    AgentDefinition {
        id: "reviewer".into(),
        backend: "fake".into(),
        execution_mode: AUTOMATED.into(),
        display_name: "Reviewer".into(),
        enabled: true,
        priority: 100,
        capabilities: Vec::new(),
        status: AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![AgentAction::Review],
    }
}

#[test]
fn app_instances_are_isolated_for_queries_and_mutations() {
    let (_one_dir, one, one_task) = app_with_task("one");
    let (_two_dir, two, two_task) = app_with_task("two");

    one.cancel(&one_task, None).unwrap();
    two.cancel(&two_task, None).unwrap();
    assert_eq!(
        one.task(&one_task).unwrap().unwrap().status.to_string(),
        "cancelled"
    );
    assert_eq!(
        two.task(&two_task).unwrap().unwrap().status.to_string(),
        "cancelled"
    );
}

#[test]
fn app_plan_uses_database_validation() {
    let (_directory, app, _task) = app_with_task("plan");
    let invalid = PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "objective".into(),
        tasks: vec![PlannedTask {
            local_id: "duplicate".into(),
            title: "one".into(),
            objective: "objective".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            depends_on: vec!["missing".into()],
            capabilities: vec![],
            scope_mode: None,
            context_files: vec![],
            expected_changes: vec![],
            unchanged: vec!["unrelated behavior".into()],
            acceptance_criteria: vec!["behavior works".into()],
            required_tests: vec!["production path test".into()],
            validation: vec!["cargo test".into()],
            execution_hints: Default::default(),
        }],
        assumptions: vec![],
        risks: vec![],
        questions: vec![],
    };
    assert!(app.apply_plan(&invalid).is_err());
}

#[test]
fn blocked_failed_task_can_be_requeued_without_losing_run_history() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    let project = db.create_project("recovery").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let run = db.create_agent_run(project, &task, "agent").unwrap();
    db.update_agent_run_status(run, "failed", Some("validation failed"))
        .unwrap();
    db.update_task_status(&task, orc::task::TaskStatus::Blocked)
        .unwrap();

    let app = OrcApp::open(&database, directory.path()).unwrap();
    app.requeue(&task).unwrap();

    assert_eq!(
        app.task(&task).unwrap().unwrap().status,
        orc::task::TaskStatus::Backlog
    );
    assert_eq!(app.runs_workspace(10, 10).unwrap().runs[0].status, "failed");
    assert!(
        app.lifecycle_events(10)
            .unwrap()
            .iter()
            .any(|event| event.kind == "task_requeue" && event.task_id.as_deref() == Some(&task))
    );
}

#[test]
fn app_subscription_receives_domain_events_in_order_without_replay() {
    let (_directory, app, task) = app_with_task("events");
    let subscription = app.subscribe();

    app.requeue(&task).unwrap();
    let first = subscription.recv().unwrap();
    assert!(
        matches!(first, orc::events::AppEvent::TaskLifecycle(ref event) if event.kind == "task_requeue")
    );
    assert_eq!(subscription.try_recv(), Err(TryRecvError::Empty));

    let second_subscription = app.subscribe();
    assert_eq!(second_subscription.try_recv(), Err(TryRecvError::Empty));
    assert!(matches!(subscription.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn disconnected_subscriber_does_not_affect_other_subscribers_or_operation() {
    let (_directory, app, task) = app_with_task("disconnect");
    let dropped = app.subscribe();
    let remaining = app.subscribe();
    drop(dropped);

    app.requeue(&task).unwrap();
    assert!(remaining.recv().is_ok());
}

#[test]
fn persisted_history_reconstructs_without_subscriber() {
    let (directory, app, task) = app_with_task("persisted");
    app.requeue(&task).unwrap();
    let history = app.lifecycle_events(10).unwrap();
    assert!(
        history
            .iter()
            .any(|event| event.task_id.as_deref() == Some(&task))
    );
    drop(app);
    let reopened = OrcApp::open(directory.path().join("state.sqlite"), directory.path()).unwrap();
    assert_eq!(
        reopened.task(&task).unwrap().unwrap().status.to_string(),
        "backlog"
    );
    assert!(!reopened.lifecycle_events(10).unwrap().is_empty());
}

#[test]
fn review_inspection_paths_are_provider_free_and_no_review_output_is_useful() {
    let (directory, app, task) = app_with_task("inspection");
    let backend = CountingReviewBackend(AtomicUsize::new(0));

    let latest = app.review(&task).unwrap();
    let full_json = serde_json::to_string(&app.review(&task).unwrap()).unwrap();
    let history = app.review_history(&task).unwrap();

    assert!(latest.automated_reviews.is_empty());
    assert!(orc::review::format_review(&latest).contains("Automated review  None persisted"));
    assert!(full_json.contains("\"automated_reviews\":[]"));
    assert!(history.is_empty());
    assert_eq!(backend.calls(), 0);

    let error = app.review_for_run(&task, 999_999).unwrap_err().to_string();
    assert!(error.contains("not found for task"));
    assert_eq!(backend.calls(), 0);
    drop(directory);
}

#[test]
fn persisted_full_review_survives_restart_through_orc_app_read_model() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("restart").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let expected = review_result("distinctive", "REVISE");
    let evidence = r#"{"command":"cargo test distinctive","passed":false}"#;
    let run = persist_review(&db, project, &task, &expected, evidence);
    drop(db);

    let reopened = OrcApp::open(&db_path, directory.path()).unwrap();
    let summary = reopened.review_for_run(&task, run).unwrap();
    let actual = &summary.automated_reviews[0];
    assert_eq!(actual.run_id, run);
    assert_eq!(actual.agent, "reviewer");
    assert_eq!(actual.status, "completed");
    assert_eq!(actual.model.as_deref(), Some("test-model"));
    assert_eq!(actual.reasoning_effort, Some(ReasoningEffort::High));
    assert_eq!(actual.verdict, expected.verdict);
    assert_eq!(actual.severity, expected.severity);
    assert_eq!(actual.findings, expected.findings);
    assert_eq!(actual.blocking_findings, expected.blocking_findings);
    assert_eq!(actual.non_blocking_findings, expected.non_blocking_findings);
    assert_eq!(actual.revision_feedback, expected.revision_feedback);
    assert_eq!(actual.validation_evidence.as_deref(), Some(evidence));
    assert!(!actual.started_at.is_empty());
    assert!(actual.finished_at.is_some());
}

#[test]
fn review_history_is_complete_chronological_and_latest_is_newest() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("history").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let first = persist_review(
        &db,
        project,
        &task,
        &review_result("a", "REVISE"),
        "evidence-a",
    );
    let second = persist_review(
        &db,
        project,
        &task,
        &review_result("b", "REJECT"),
        "evidence-b",
    );
    let third = persist_review(
        &db,
        project,
        &task,
        &review_result("c", "PASS"),
        "evidence-c",
    );
    drop(db);
    let app = OrcApp::open(&db_path, directory.path()).unwrap();

    let history = app.review_history(&task).unwrap();
    assert_eq!(
        history
            .iter()
            .map(|review| review.run_id)
            .collect::<Vec<_>>(),
        vec![first, second, third]
    );
    assert_eq!(history.len(), 3);
    let latest = app.review(&task).unwrap();
    assert_eq!(latest.automated_reviews.last().unwrap().run_id, third);
    assert!(orc::review::format_review(&latest).contains(&format!("Automated review #{third}")));
}

#[test]
fn historical_review_is_task_scoped_and_keeps_its_own_evidence() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("historical").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let other = db
        .insert_task(
            project,
            "other",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let a = review_result("a", "REVISE");
    let b = review_result("b", "PASS");
    let run_a = persist_review(&db, project, &task, &a, "validation-a");
    let run_b = persist_review(&db, project, &task, &b, "validation-b");
    let other_run = persist_review(
        &db,
        project,
        &other,
        &review_result("other", "PASS"),
        "validation-other",
    );
    drop(db);
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = CountingReviewBackend(AtomicUsize::new(0));

    let selected = app.review_for_run(&task, run_a).unwrap();
    let review = &selected.automated_reviews[0];
    assert_eq!(review.run_id, run_a);
    assert_ne!(review.run_id, run_b);
    assert_eq!(review.verdict, a.verdict);
    assert_eq!(review.severity, a.severity);
    assert_eq!(review.findings, a.findings);
    assert_eq!(review.revision_feedback, a.revision_feedback);
    assert_eq!(review.validation_evidence.as_deref(), Some("validation-a"));
    assert_ne!(review.validation_evidence.as_deref(), Some("validation-b"));
    let error = app
        .review_for_run(&task, other_run)
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not belong to task"));
    assert_eq!(backend.calls(), 0);
}

#[test]
fn explicit_automated_review_still_invokes_backend_once() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("automated").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.insert_agent(&reviewer()).unwrap();
    drop(db);
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = CountingReviewBackend(AtomicUsize::new(0));

    let (_, result) = app
        .automated_review_with_backend(&task, &ActionOverrides::default(), &backend)
        .unwrap();

    assert_eq!(result.verdict, "PASS");
    assert_eq!(backend.calls(), 1);
    assert_eq!(app.review_history(&task).unwrap().len(), 1);
}

#[test]
fn failed_database_purge_does_not_remove_task_worktree() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    let project = db.create_project("purge-lock").unwrap();
    let task = db
        .insert_task(project, "purge", "purge", "developer", TaskPriority::Normal)
        .unwrap();
    db.update_task_status(&task, orc::task::TaskStatus::Cancelled)
        .unwrap();
    let run = db.create_agent_run(project, &task, "agent").unwrap();
    db.update_agent_run_status(run, "completed", Some("done"))
        .unwrap();
    db.store_worktree_metadata(run, &task, "branch", ".orc/worktrees/purge")
        .unwrap();
    let worktree = directory
        .path()
        .join(orc::git::worktree_path_for_task(&task));
    std::fs::create_dir_all(&worktree).unwrap();
    let app = OrcApp::open(&database, directory.path()).unwrap();
    let lock = Connection::open(&database).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(app.purge_task(&task, true).is_err());
    assert!(worktree.exists());
    assert!(app.task(&task).unwrap().is_some());
    lock.execute_batch("ROLLBACK").unwrap();
}
