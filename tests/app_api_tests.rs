use orc::app::OrcApp;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides, ReviewResult};
use orc::lead::LeadDecisionKind;
use orc::protocol::{ExecutionHints, PROTOCOL_VERSION, PlanResponse, PlannedTask};
use orc::registry::{AUTOMATED, AVAILABLE, AgentAction, AgentDefinition, ReasoningEffort};
use orc::storage::db::LeadDecisionMetadata;
use orc::storage::{AgentRunExecution, Database};
use orc::task::{CreateTaskInput, TaskPriority, TaskScopeMode};
use orc::validation::test_helpers::FakeValidationRunner;
use rusqlite::Connection;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::TryRecvError;
use tempfile::tempdir;

struct CountingPlanner(AtomicUsize);

impl ActionBackend for CountingPlanner {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<ReasoningEffort>,
    ) -> anyhow::Result<ActionExecution> {
        assert_eq!(action, AgentAction::Plan);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ActionExecution {
            output: serde_json::json!({
                "protocol_version": PROTOCOL_VERSION, "objective": "requested", "assumptions": [],
                "risks": [], "questions": [], "tasks": []
            })
            .to_string(),
            token_usage: None,
        })
    }
}

struct OutputPlanner {
    calls: AtomicUsize,
    output: String,
}

struct DecisionChangingPlanner {
    db_path: std::path::PathBuf,
    project_id: i64,
    calls: AtomicUsize,
}

impl ActionBackend for DecisionChangingPlanner {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<ReasoningEffort>,
    ) -> anyhow::Result<ActionExecution> {
        assert_eq!(action, AgentAction::Plan);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Database::open(&self.db_path)?.consume_pending_lead_decision(self.project_id)?;
        Ok(ActionExecution {
            output: serde_json::json!({
                "protocol_version": PROTOCOL_VERSION, "objective": "requested",
                "assumptions": [], "risks": [], "questions": [], "tasks": []
            })
            .to_string(),
            token_usage: None,
        })
    }
}

fn proposal(local_id: &str, depends_on: Vec<&str>) -> PlannedTask {
    PlannedTask {
        local_id: local_id.into(),
        title: format!("{local_id} title"),
        objective: format!("{local_id} objective"),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: depends_on.into_iter().map(str::to_owned).collect(),
        capabilities: vec!["code".into()],
        scope_mode: Some(TaskScopeMode::Focused),
        context_files: vec!["src/lib.rs".into()],
        expected_changes: vec!["src/lib.rs".into()],
        unchanged: vec!["task state".into()],
        acceptance_criteria: vec!["works".into()],
        required_tests: vec!["production test".into()],
        validation: vec!["cargo test".into()],
        execution_hints: ExecutionHints {
            class: Some("code".into()),
            model: None,
            effort: Some("low".into()),
            effort_reason: Some("isolated and well understood".into()),
        },
        risk_factors: vec![],
    }
}

impl ActionBackend for OutputPlanner {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<ReasoningEffort>,
    ) -> anyhow::Result<ActionExecution> {
        assert_eq!(action, AgentAction::Plan);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ActionExecution {
            output: self.output.clone(),
            token_usage: None,
        })
    }
}

fn planner() -> AgentDefinition {
    AgentDefinition {
        id: "planner".into(),
        backend: "fake".into(),
        execution_mode: AUTOMATED.into(),
        display_name: "Planner".into(),
        enabled: true,
        priority: 1,
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
        actions: vec![AgentAction::Plan],
    }
}

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

#[test]
fn workflow_state_is_project_scoped_and_read_only() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let current = db.create_project("current").unwrap();
    let other = db.create_project("other").unwrap();
    let current_task = db
        .insert_task(
            current,
            "current task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&current_task, orc::task::TaskStatus::Active)
        .unwrap();
    let other_task = db
        .insert_task(
            other,
            "other task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&other_task, orc::task::TaskStatus::Blocked)
        .unwrap();
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let before = app.workflow_state().unwrap();
    let after = app.workflow_state().unwrap();
    assert_eq!(before.position, "task_execution");
    assert_eq!(before.tasks.len(), 1);
    assert_eq!(before.tasks[0].id, current_task);
    assert_eq!(before.tasks, after.tasks);
    assert_eq!(before.position, after.position);
}

#[test]
fn workflow_state_derives_each_task_lifecycle_position() {
    let cases = [
        (orc::task::TaskStatus::Review, "task_review"),
        (
            orc::task::TaskStatus::AcceptanceReady,
            "task_acceptance_ready",
        ),
        (
            orc::task::TaskStatus::RevisionRequired,
            "task_revision_required",
        ),
        (orc::task::TaskStatus::Active, "task_execution"),
        (orc::task::TaskStatus::Blocked, "blocked"),
        (orc::task::TaskStatus::Done, "complete"),
    ];

    for (status, expected) in cases {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("state.sqlite");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("workflow").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        db.update_task_status(&task, status).unwrap();
        drop(db);

        let app = OrcApp::open(&db_path, directory.path()).unwrap();
        let state = app.workflow_state().unwrap();
        assert_eq!(state.position, expected);
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks[0].status, status);
    }
}

#[test]
fn workflow_state_derives_lead_and_planner_positions() {
    for (kind, expected) in [
        (LeadDecisionKind::DirectTasks, "lead_decision"),
        (LeadDecisionKind::PlanRequired, "planner_required"),
        (
            LeadDecisionKind::UserDecisionRequired,
            "user_decision_required",
        ),
    ] {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("state.sqlite");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("workflow").unwrap();
        db.record_lead_decision(
            project,
            &kind,
            &serde_json::json!({"kind": "workflow"}),
            LeadDecisionMetadata {
                snapshot: "snapshot",
                run_id: None,
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
        drop(db);

        let app = OrcApp::open(&db_path, directory.path()).unwrap();
        let state = app.workflow_state().unwrap();
        assert_eq!(state.position, expected);
        assert_eq!(state.lead_decisions.len(), 1);
        assert_eq!(
            state.user_decisions.len(),
            usize::from(kind == LeadDecisionKind::UserDecisionRequired)
        );
    }
}

#[test]
fn workflow_state_derives_plan_review_revision_and_ready_positions() {
    for (status, expected) in [
        ("proposed", "plan_review"),
        ("revision_requested", "plan_review"),
        ("applied", "tasks_ready"),
    ] {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("state.sqlite");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("workflow").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let decision = db
            .record_lead_decision(
                project,
                &LeadDecisionKind::PlanRequired,
                &serde_json::json!({}),
                LeadDecisionMetadata {
                    snapshot: "snapshot",
                    run_id: None,
                    source_request: "request",
                    summary: "summary",
                },
            )
            .unwrap();
        let run = db
            .create_agent_run_with_execution(
                project,
                &task,
                "planner",
                AUTOMATED,
                AgentRunExecution {
                    class: "plan",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        let response = PlanResponse {
            protocol_version: PROTOCOL_VERSION,
            objective: "objective".into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        };
        let plan = db.store_plan(project, decision, run, &response).unwrap();
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE plans SET status = ?1 WHERE id = ?2",
                (&status, plan),
            )
            .unwrap();
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE lead_decisions SET status = 'consumed' WHERE id = ?1",
                [decision],
            )
            .unwrap();
        Connection::open(&db_path)
            .unwrap()
            .execute(
                "UPDATE agent_runs SET status = 'completed' WHERE id = ?1",
                [run],
            )
            .unwrap();
        drop(db);

        let state = OrcApp::open(&db_path, directory.path())
            .unwrap()
            .workflow_state()
            .unwrap();
        assert_eq!(state.position, expected);
        assert_eq!(state.plans.len(), 1);
    }
}

#[test]
fn operator_cancellation_is_persistent_scoped_and_single_use() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("cancel").unwrap();
    let other_project = db.create_project("other project").unwrap();
    let decision = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::UserDecisionRequired,
            &serde_json::json!({"choice": "x"}),
            LeadDecisionMetadata {
                snapshot: "snapshot",
                run_id: None,
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
    let other_decision = db
        .record_lead_decision(
            other_project,
            &LeadDecisionKind::UserDecisionRequired,
            &serde_json::json!({"choice": "other"}),
            LeadDecisionMetadata {
                snapshot: "other snapshot",
                run_id: None,
                source_request: "other request",
                summary: "other summary",
            },
        )
        .unwrap();
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    assert!(app.cancel_lead_decision(other_decision, None).is_err());
    assert_eq!(
        Database::open(&db_path)
            .unwrap()
            .pending_lead_decision(other_project)
            .unwrap()
            .unwrap()
            .id,
        other_decision
    );
    let cancelled = app
        .cancel_lead_decision(decision, Some("operator stopped"))
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(!cancelled.actionable);
    assert_eq!(cancelled.resolution.as_deref(), Some("operator stopped"));
    assert!(app.pending_lead_decision().unwrap().is_none());
    assert!(app.cancel_lead_decision(decision, None).is_err());

    let reopened = OrcApp::open(&db_path, directory.path()).unwrap();
    let history = reopened.lead_decisions().unwrap();
    assert_eq!(history[0].id, decision);
    assert_eq!(history[0].status, "cancelled");
    assert_eq!(reopened.workflow_state().unwrap().position, "lead_decision");
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
fn pending_plan_run_invokes_once_persists_lineage_and_is_visible_after_reopen() {
    let directory = tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("planner").unwrap();
    db.insert_agent(&planner()).unwrap();
    let decision = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::PlanRequired,
            &serde_json::json!({"kind":"PLAN_REQUIRED"}),
            LeadDecisionMetadata {
                snapshot: "before",
                run_id: None,
                source_request: "requested",
                summary: "make a plan",
            },
        )
        .unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = CountingPlanner(AtomicUsize::new(0));
    let result = app
        .run_pending_plan_with_backend(
            &ActionOverrides {
                agent_id: Some("planner".into()),
                ..Default::default()
            },
            &backend,
        )
        .unwrap();
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    assert_eq!(result.lead_decision_id, decision);
    let reopened = Database::open(&db_path).unwrap();
    let plan = reopened.get_plan(result.plan_id).unwrap().unwrap();
    assert_eq!(plan.source_lead_decision_id, decision);
    assert_eq!(plan.source_planner_run_id, result.planner_run_id);
    assert!(reopened.pending_lead_decision(project).unwrap().is_none());
    assert_eq!(reopened.list_tasks().unwrap().len(), 0);
}

#[test]
fn cancelling_plan_review_closes_linked_plan_and_blocks_consumption() {
    let directory = tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("cancel review").unwrap();
    db.insert_agent(&planner()).unwrap();
    let source = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::PlanRequired,
            &serde_json::json!({"kind":"PLAN_REQUIRED"}),
            LeadDecisionMetadata {
                snapshot: "before",
                run_id: None,
                source_request: "requested",
                summary: "make a plan",
            },
        )
        .unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = CountingPlanner(AtomicUsize::new(0));
    let run = app
        .run_pending_plan_with_backend(
            &ActionOverrides {
                agent_id: Some("planner".into()),
                ..Default::default()
            },
            &backend,
        )
        .unwrap();
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    let decision = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::UserDecisionRequired,
            &serde_json::json!({"choice":"approve"}),
            LeadDecisionMetadata {
                snapshot: "review",
                run_id: Some(run.planner_run_id),
                source_request: "review",
                summary: "review",
            },
        )
        .unwrap();
    let review = db
        .record_plan_review(
            run.plan_id,
            run.planner_run_id,
            decision,
            &LeadDecisionKind::UserDecisionRequired,
            "review gate",
        )
        .unwrap();
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    app.cancel_plan_review(review, Some("operator stopped"))
        .unwrap();
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.workflow_state().unwrap().plans[0].status,
        orc::storage::db::PlanStatus::Cancelled
    );
    assert_eq!(
        app.lead_decisions()
            .unwrap()
            .iter()
            .find(|d| d.id == decision)
            .unwrap()
            .status,
        "cancelled"
    );
    assert_eq!(app.workflow_state().unwrap().plan_reviews.len(), 1);
    assert!(app.cancel_plan_review(review, None).is_err());
    assert!(app.apply_approved_plan().is_err());
    assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    assert!(
        Database::open(&db_path)
            .unwrap()
            .pending_lead_decision(project)
            .unwrap()
            .is_none()
    );
    assert_ne!(source, decision);
}

#[test]
fn plan_run_preserves_canonical_proposals_and_dependencies_without_creating_tasks() {
    let directory = tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("planner").unwrap();
    db.insert_agent(&planner()).unwrap();
    db.record_lead_decision(
        project,
        &LeadDecisionKind::PlanRequired,
        &serde_json::json!({"kind":"PLAN_REQUIRED"}),
        LeadDecisionMetadata {
            snapshot: "before",
            run_id: None,
            source_request: "requested",
            summary: "summary",
        },
    )
    .unwrap();
    let response = PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "requested".into(),
        assumptions: vec![],
        risks: vec![],
        questions: vec![],
        tasks: vec![proposal("first", vec![]), proposal("second", vec!["first"])],
    };
    let backend = OutputPlanner {
        calls: AtomicUsize::new(0),
        output: serde_json::to_string(&response).unwrap(),
    };
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let result = app
        .run_pending_plan_with_backend(
            &ActionOverrides {
                agent_id: Some("planner".into()),
                ..Default::default()
            },
            &backend,
        )
        .unwrap();
    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        reopened.get_plan(result.plan_id).unwrap().unwrap().response,
        response
    );
    assert_eq!(
        reopened.list_plan_dependencies(result.plan_id).unwrap(),
        vec![("second".into(), "first".into())]
    );
    assert!(reopened.list_tasks().unwrap().is_empty());

    let approval = reopened
        .record_lead_decision(
            project,
            &LeadDecisionKind::Approve,
            &serde_json::json!({"kind":"APPROVE"}),
            LeadDecisionMetadata {
                snapshot: "approved",
                run_id: Some(result.planner_run_id),
                source_request: "explicit plan review",
                summary: "approve",
            },
        )
        .unwrap();
    reopened
        .record_plan_review(
            result.plan_id,
            result.planner_run_id,
            approval,
            &LeadDecisionKind::Approve,
            "approved for explicit application",
        )
        .unwrap();
    let mapping = OrcApp::open(&db_path, directory.path())
        .unwrap()
        .apply_approved_plan()
        .unwrap();
    assert_eq!(mapping.len(), 2);
    assert_eq!(reopened.list_tasks().unwrap().len(), 2);
    let events = reopened.list_lifecycle_events(20).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "task_created")
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "plan_applied")
            .count(),
        1
    );
}

#[test]
fn direct_task_creation_does_not_require_planner() {
    let directory = tempdir().unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    db.create_project("direct").unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let task = app
        .create_task(CreateTaskInput {
            title: "Direct task".into(),
            objective: "Create without planning".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            required_capabilities: Vec::new(),
            scope_mode: None,
            context_files: Vec::new(),
            expected_changes: Vec::new(),
            dependencies: Vec::new(),
        })
        .unwrap();
    assert!(db.get_task(&task).unwrap().is_some());
    assert!(
        db.list_agent_runs(db.get_project_id().unwrap().unwrap(), 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        db.list_lifecycle_events_for_task(&task, 10)
            .unwrap()
            .iter()
            .any(|event| event.kind == "task_created")
    );
}

#[test]
fn plan_run_rejects_missing_superseded_and_consumed_decisions_before_invocation() {
    let cases = ["missing", "superseded", "consumed"];
    for case in cases {
        let directory = tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".orc")).unwrap();
        std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
        let db_path = directory.path().join("state.sqlite");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("planner").unwrap();
        db.insert_agent(&planner()).unwrap();
        if case != "missing" {
            db.record_lead_decision(
                project,
                &LeadDecisionKind::PlanRequired,
                &serde_json::json!({}),
                LeadDecisionMetadata {
                    snapshot: "s",
                    run_id: None,
                    source_request: "r",
                    summary: "s",
                },
            )
            .unwrap();
        }
        if case == "superseded" {
            db.record_lead_decision(
                project,
                &LeadDecisionKind::DirectTasks,
                &serde_json::json!({}),
                LeadDecisionMetadata {
                    snapshot: "s",
                    run_id: None,
                    source_request: "r",
                    summary: "s",
                },
            )
            .unwrap();
        }
        let app = OrcApp::open(&db_path, directory.path()).unwrap();
        let backend = CountingPlanner(AtomicUsize::new(0));
        if case == "consumed" {
            app.run_pending_plan_with_backend(
                &ActionOverrides {
                    agent_id: Some("planner".into()),
                    ..Default::default()
                },
                &backend,
            )
            .unwrap();
        }
        assert!(
            app.run_pending_plan_with_backend(
                &ActionOverrides {
                    agent_id: Some("planner".into()),
                    ..Default::default()
                },
                &backend
            )
            .is_err()
        );
        assert_eq!(
            backend.0.load(Ordering::SeqCst),
            if case == "consumed" { 1 } else { 0 }
        );
        assert!(
            Database::open(&db_path)
                .unwrap()
                .list_tasks()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn plan_run_rejects_non_actionable_decisions_without_planner_or_state_changes() {
    for kind in [
        LeadDecisionKind::DirectTasks,
        LeadDecisionKind::UserDecisionRequired,
    ] {
        let directory = tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".orc")).unwrap();
        std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
        let db_path = directory.path().join("state.sqlite");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("planner").unwrap();
        db.insert_agent(&planner()).unwrap();
        db.record_lead_decision(
            project,
            &kind,
            &serde_json::json!({"kind":"other"}),
            LeadDecisionMetadata {
                snapshot: "before",
                run_id: None,
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
        let app = OrcApp::open(&db_path, directory.path()).unwrap();
        let backend = CountingPlanner(AtomicUsize::new(0));
        assert!(
            app.run_pending_plan_with_backend(
                &ActionOverrides {
                    agent_id: Some("planner".into()),
                    ..Default::default()
                },
                &backend
            )
            .is_err()
        );
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
        assert!(
            Database::open(&db_path)
                .unwrap()
                .list_plan_history(project)
                .unwrap()
                .is_empty()
        );
        assert!(
            Database::open(&db_path)
                .unwrap()
                .list_tasks()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn malformed_planner_output_is_rejected_without_partial_plan_or_task_state() {
    let directory = tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("planner").unwrap();
    db.insert_agent(&planner()).unwrap();
    let decision = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::PlanRequired,
            &serde_json::json!({"kind":"PLAN_REQUIRED"}),
            LeadDecisionMetadata {
                snapshot: "before",
                run_id: None,
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = OutputPlanner {
        calls: AtomicUsize::new(0),
        output: "not json".into(),
    };
    assert!(
        app.run_pending_plan_with_backend(
            &ActionOverrides {
                agent_id: Some("planner".into()),
                ..Default::default()
            },
            &backend
        )
        .is_err()
    );
    let db = Database::open(&db_path).unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(db.list_plan_history(project).unwrap().is_empty());
    assert!(db.list_tasks().unwrap().is_empty());
    assert_eq!(
        db.pending_lead_decision(project).unwrap().unwrap().id,
        decision
    );
}

#[test]
fn changed_pending_decision_rolls_back_plan_atomically() {
    let directory = tempdir().unwrap();
    std::fs::create_dir(directory.path().join(".orc")).unwrap();
    std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
    let db_path = directory.path().join("state.sqlite");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("planner").unwrap();
    db.insert_agent(&planner()).unwrap();
    db.record_lead_decision(
        project,
        &LeadDecisionKind::PlanRequired,
        &serde_json::json!({"kind":"PLAN_REQUIRED"}),
        LeadDecisionMetadata {
            snapshot: "before",
            run_id: None,
            source_request: "request",
            summary: "summary",
        },
    )
    .unwrap();
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = DecisionChangingPlanner {
        db_path: db_path.clone(),
        project_id: project,
        calls: AtomicUsize::new(0),
    };
    assert!(
        app.run_pending_plan_with_backend(
            &ActionOverrides {
                agent_id: Some("planner".into()),
                ..Default::default()
            },
            &backend
        )
        .is_err()
    );
    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(reopened.list_plan_history(project).unwrap().is_empty());
    assert!(reopened.list_tasks().unwrap().is_empty());
    assert!(reopened.pending_lead_decision(project).unwrap().is_none());
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
            risk_factors: vec![],
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
    db.update_task_status(&task, orc::task::TaskStatus::Review)
        .unwrap();
    drop(db);
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let backend = CountingReviewBackend(AtomicUsize::new(0));

    let (_, result) = app
        .automated_review_with_backend(
            &task,
            &ActionOverrides::default(),
            &backend,
            &FakeValidationRunner::success(),
        )
        .unwrap();

    assert_eq!(result.verdict, "PASS");
    assert_eq!(
        app.task(&task).unwrap().unwrap().status,
        orc::task::TaskStatus::AcceptanceReady
    );
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
