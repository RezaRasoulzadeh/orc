use orc::Database;
use orc::app::OrcApp;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides};
use orc::lead::{
    CodexLeadBackend, LeadBackend, LeadBackendResponse, LeadContext, LeadDecision,
    LeadDecisionKind, LeadProposalKind, LeadProposalStatus, LeadProviderConfig, LeadRole,
};
use orc::protocol::PlannedTask;
use orc::registry::{AgentDefinition, ReasoningEffort};
use orc::task::{TaskPriority, TaskScopeMode};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::tempdir;

struct FakeLead {
    contexts: RefCell<Vec<LeadContext>>,
    response: RefCell<Option<LeadBackendResponse>>,
}

impl LeadBackend for FakeLead {
    fn invoke(&self, context: &LeadContext, _: &str) -> Result<LeadBackendResponse, String> {
        self.contexts.borrow_mut().push(context.clone());
        self.response
            .borrow_mut()
            .take()
            .ok_or_else(|| "fake response already used".into())
    }
}

struct FailingLead;

impl LeadBackend for FailingLead {
    fn invoke(&self, _: &LeadContext, _: &str) -> Result<LeadBackendResponse, String> {
        Err("provider unavailable".into())
    }
}

struct MalformedProviderLead;

impl LeadBackend for MalformedProviderLead {
    fn invoke(&self, _: &LeadContext, _: &str) -> Result<LeadBackendResponse, String> {
        CodexLeadBackend::parse_response("{not valid json")
    }
}

struct InvalidDecisionLead;

impl LeadBackend for InvalidDecisionLead {
    fn invoke(&self, _: &LeadContext, _: &str) -> Result<LeadBackendResponse, String> {
        CodexLeadBackend::parse_response(
            r#"{"message":"bad","decision":{"kind":"NOT_A_DECISION","details":{}}}"#,
        )
    }
}

struct CountingActionBackend(AtomicUsize);

impl ActionBackend for CountingActionBackend {
    fn invoke(
        &self,
        _: &AgentDefinition,
        _: orc::registry::AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<ReasoningEffort>,
    ) -> anyhow::Result<ActionExecution> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ActionExecution {
            output: String::new(),
            token_usage: None,
        })
    }
}

#[test]
fn codex_lead_transport_normalizes_nullable_plan_lists() {
    let response = CodexLeadBackend::parse_response(
        r#"{
            "message":"plan suggestion",
            "proposals":[{
                "kind":"plan",
                "details":{
                    "protocol_version":1,
                    "objective":"Repair structured transport",
                    "assumptions":null,
                    "risks":null,
                    "questions":null,
                    "tasks":null,
                    "local_id":"ignored flattened task payload",
                    "task_id":null,
                    "feedback":null
                }
            }],
            "decision":{"kind":"PLAN_REQUIRED","details":{"tasks":null}}
        }"#,
    )
    .unwrap();

    let LeadProposalKind::Plan(plan) = &response.proposals[0] else {
        panic!("expected a plan proposal");
    };
    assert!(plan.assumptions.is_empty());
    assert!(plan.risks.is_empty());
    assert!(plan.questions.is_empty());
    assert!(plan.tasks.is_empty());
    assert_eq!(
        response.decision.unwrap().kind,
        LeadDecisionKind::PlanRequired
    );
}

#[test]
fn codex_lead_transport_normalizes_only_optional_task_lists() {
    let response = CodexLeadBackend::parse_response(
        r#"{
            "message":"task suggestion",
            "proposals":[{
                "kind":"task",
                "details":{
                    "local_id":"transport-fix",
                    "title":"Fix transport",
                    "objective":"Normalize nullable provider fields",
                    "role":"developer",
                    "priority":"normal",
                    "depends_on":null,
                    "capabilities":null,
                    "scope_mode":"project",
                    "context_files":null,
                    "expected_changes":["src/lead.rs"],
                    "unchanged":["Lead domain types"],
                    "acceptance_criteria":["Null optional lists deserialize"],
                    "required_tests":["cargo test --test lead_tests"],
                    "validation":["cargo test --test lead_tests"],
                    "execution_hints":{"class":null,"model":null,"effort":"low","effort_reason":"focused boundary fix"},
                    "risk_factors":null,
                    "tasks":null,
                    "task_id":null,
                    "feedback":null
                }
            }],
            "decision":{"kind":"DIRECT_TASKS","details":{"tasks":null}}
        }"#,
    )
    .unwrap();

    let LeadProposalKind::Task(task) = &response.proposals[0] else {
        panic!("expected a task proposal");
    };
    assert!(task.depends_on.is_empty());
    assert!(task.capabilities.is_empty());
    assert!(task.context_files.is_empty());
    assert!(task.risk_factors.is_empty());
}

#[test]
fn codex_lead_transport_normalizes_a_null_proposals_list() {
    let response = CodexLeadBackend::parse_response(
        r#"{"message":"assessment","proposals":null,"decision":{"kind":"PLAN_REQUIRED","details":{}}}"#,
    )
    .unwrap();

    assert!(response.proposals.is_empty());
}

#[test]
fn codex_lead_transport_rejects_null_or_mismatched_proposal_payloads() {
    let null_payload = CodexLeadBackend::parse_response(
        r#"{"message":"bad","proposals":[{"kind":"plan","details":null}],"decision":{"kind":"PLAN_REQUIRED","details":{}}}"#,
    )
    .unwrap_err();
    assert!(null_payload.contains("plan proposal requires a non-null details payload"));

    let mismatched = CodexLeadBackend::parse_response(
        r#"{
            "message":"bad",
            "proposals":[{
                "kind":"revision",
                "details":{
                    "local_id":"task-shaped",
                    "title":"Wrong payload",
                    "task_id":null,
                    "feedback":null
                }
            }],
            "decision":{"kind":"PLAN_REQUIRED","details":{}}
        }"#,
    )
    .unwrap_err();
    assert!(mismatched.contains("invalid revision proposal payload"));

    let null_required_list = CodexLeadBackend::parse_response(
        r#"{
            "message":"bad",
            "proposals":[{
                "kind":"task",
                "details":{
                    "local_id":"transport-fix",
                    "title":"Fix transport",
                    "objective":"Normalize nullable provider fields",
                    "role":"developer",
                    "priority":"normal",
                    "depends_on":null,
                    "capabilities":null,
                    "scope_mode":"project",
                    "context_files":null,
                    "expected_changes":["src/lead.rs"],
                    "unchanged":["Lead domain types"],
                    "acceptance_criteria":null,
                    "required_tests":["cargo test --test lead_tests"],
                    "validation":["cargo test --test lead_tests"],
                    "execution_hints":{"effort":"low","effort_reason":"focused boundary fix"},
                    "risk_factors":null
                }
            }],
            "decision":{"kind":"PLAN_REQUIRED","details":{}}
        }"#,
    )
    .unwrap_err();
    assert!(null_required_list.contains("invalid task proposal payload"));
}

#[test]
fn new_project_intake_validates_objective_and_persists_read_only_lead_decision() {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".orc")).unwrap();
    let path = dir.path().join(".orc/orc.db");
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .unwrap();
    let db = Database::init(&path).unwrap();
    db.create_project("new-project").unwrap();
    let mut lead = codex_agent("/profiles/lead");
    lead.actions = vec![orc::registry::AgentAction::Lead];
    db.insert_agent(&lead).unwrap();
    db.set_lead_provider_config(&LeadProviderConfig {
        agent_id: lead.id.clone(),
        model: None,
        reasoning_effort: None,
    })
    .unwrap();
    drop(db);

    struct IntakeBackend {
        input: RefCell<Option<String>>,
    }
    impl ActionBackend for IntakeBackend {
        fn invoke(
            &self,
            _: &AgentDefinition,
            action: orc::registry::AgentAction,
            input: &str,
            _: Option<&str>,
            _: Option<ReasoningEffort>,
        ) -> anyhow::Result<ActionExecution> {
            assert_eq!(action, orc::registry::AgentAction::Lead);
            self.input.replace(Some(input.to_owned()));
            Ok(ActionExecution {
                output: r#"{"message":"intake","proposals":[],"decision":{"kind":"PLAN_REQUIRED","details":{"next":"operator"}}}"#.into(),
                token_usage: None,
            })
        }
    }

    let app = OrcApp::open(&path, dir.path()).unwrap();
    let backend = IntakeBackend {
        input: RefCell::new(None),
    };
    app.new_project_intake_with_backend(
        "Ship the first release",
        &ActionOverrides::default(),
        &backend,
    )
    .unwrap();
    let input = backend.input.borrow().clone().unwrap();
    let lead_input = input
        .split("## Authoritative Orc packet")
        .nth(1)
        .and_then(|value| value.find('{').map(|index| &value[index..]))
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_str(lead_input).unwrap();
    let request: serde_json::Value =
        serde_json::from_str(envelope["request"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(request["kind"], "new_project_intake");
    assert_eq!(request["objective"], "Ship the first release");
    let snapshot = request["discovery_snapshot"].as_object().unwrap();
    assert_eq!(snapshot["project"]["name"], "new-project");
    assert!(snapshot.contains_key("repository"));
    assert!(snapshot.contains_key("architecture"));
    assert!(snapshot.contains_key("task_state"));
    assert_eq!(app.tasks().unwrap().len(), 0);
    assert!(app.pending_lead_decision().unwrap().is_some());
    let connection = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM plans", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(
        app.new_project_intake_with_backend("  ", &ActionOverrides::default(), &backend)
            .is_err()
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM agent_runs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn plan_review_rejects_invalid_plan_before_lead_run_or_review_persistence() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("review.sqlite");
    let db = Database::init(&db_path).unwrap();
    db.create_project("review").unwrap();
    let mut lead = codex_agent("/profiles/lead");
    lead.actions = vec![orc::registry::AgentAction::Lead];
    db.insert_agent(&lead).unwrap();
    drop(db);

    let app = OrcApp::open(&db_path, dir.path()).unwrap();
    let backend = CountingActionBackend(AtomicUsize::new(0));
    assert!(
        app.review_plan_with_backend(999, &ActionOverrides::default(), &backend)
            .is_err()
    );
    assert_eq!(backend.0.load(Ordering::SeqCst), 0);

    let connection = rusqlite::Connection::open(&db_path).unwrap();
    let runs: i64 = connection
        .query_row("SELECT COUNT(*) FROM agent_runs", [], |row| row.get(0))
        .unwrap();
    let reviews: i64 = connection
        .query_row("SELECT COUNT(*) FROM plan_reviews", [], |row| row.get(0))
        .unwrap();
    assert_eq!(runs, 0);
    assert_eq!(reviews, 0);
}

fn codex_agent(profile_path: &str) -> AgentDefinition {
    AgentDefinition {
        id: "project-lead".into(),
        backend: "codex".into(),
        execution_mode: "automated".into(),
        display_name: "Project Lead".into(),
        enabled: true,
        priority: 1,
        capabilities: Vec::new(),
        status: "available".into(),
        unavailable_reason: None,
        profile_path: Some(profile_path.into()),
        model: Some("configured-model".into()),
        reasoning_effort: Some(ReasoningEffort::Medium),
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![orc::registry::AgentAction::Code],
    }
}

fn planned_task(title: &str) -> PlannedTask {
    PlannedTask {
        local_id: "lead-task".into(),
        title: title.into(),
        objective: "Implement it".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: Vec::new(),
        capabilities: Vec::new(),
        scope_mode: Some(TaskScopeMode::Project),
        context_files: Vec::new(),
        expected_changes: vec!["src/lib.rs".into()],
        unchanged: vec!["unrelated behavior".into()],
        acceptance_criteria: vec!["behavior works".into()],
        required_tests: vec!["production path test".into()],
        validation: vec!["cargo test".into()],
        execution_hints: Default::default(),
        risk_factors: vec![],
    }
}

#[test]
fn invocation_uses_fresh_state_and_persists_turns_and_pending_proposals() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("alpha").unwrap();
    db.insert_task(
        project,
        "Existing",
        "Current fact",
        "developer",
        TaskPriority::Normal,
    )
    .unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();
    let backend = FakeLead {
        contexts: RefCell::new(Vec::new()),
        response: RefCell::new(Some(LeadBackendResponse {
            message: "I propose one task".into(),
            proposals: vec![LeadProposalKind::Task(planned_task("Proposed"))],
            decision: None,
        })),
    };
    let response = app.invoke_lead("What next?", &backend, 20).unwrap();
    assert_eq!(backend.contexts.borrow()[0].tasks[0].title, "Existing");
    assert_eq!(response.turn.role, LeadRole::Assistant);
    assert_eq!(response.proposals[0].status, LeadProposalStatus::Pending);
    assert_eq!(app.tasks().unwrap().len(), 1);
    drop(app);
    let reopened = OrcApp::open(&path, dir.path()).unwrap();
    let context = reopened.lead().context(20).unwrap();
    assert_eq!(context.turns.len(), 2);
    assert_eq!(context.turns[0].role, LeadRole::User);
    assert_eq!(reopened.lead().pending_proposals().unwrap().len(), 1);
}

#[test]
fn structured_decisions_are_persisted_reopened_superseded_and_explicitly_consumed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("decisions.sqlite");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("decision project").unwrap();
    let task_id = db
        .insert_task(
            project,
            "lead run task",
            "provide a run lineage",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let run_id = db.create_agent_run(project, &task_id, "lead").unwrap();
    drop(db);
    for (index, kind) in [
        LeadDecisionKind::DirectTasks,
        LeadDecisionKind::PlanRequired,
        LeadDecisionKind::UserDecisionRequired,
    ]
    .into_iter()
    .enumerate()
    {
        let app = OrcApp::open(&path, dir.path()).unwrap();
        app.lead()
            .invoke_with_run_id(
                "assess",
                &FakeLead {
                    contexts: RefCell::new(Vec::new()),
                    response: RefCell::new(Some(LeadBackendResponse {
                        message: "assessment".into(),
                        proposals: Vec::new(),
                        decision: Some(LeadDecision {
                            kind,
                            details: serde_json::json!({"operator": "next"}),
                        }),
                    })),
                },
                20,
                Some(run_id),
            )
            .unwrap();
        let pending = app.pending_lead_decision().unwrap().unwrap();
        assert_eq!(pending.kind, kind);
        assert_eq!(pending.status, "pending");
        assert!(pending.actionable && pending.snapshot.is_some());
        assert_eq!(pending.run_id, Some(run_id));
        if index > 0 {
            assert_eq!(pending.kind, kind);
        }
        drop(app);
        let reopened = OrcApp::open(&path, dir.path()).unwrap();
        let consumed = reopened.consume_pending_lead_decision().unwrap().unwrap();
        assert_eq!(consumed.status, "consumed");
        assert!(!consumed.actionable);
    }
    assert!(
        OrcApp::open(&path, dir.path())
            .unwrap()
            .pending_lead_decision()
            .unwrap()
            .is_none()
    );
}

#[test]
fn lead_decision_supersession_is_historical_and_read_only() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("supersession.sqlite");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("decision project").unwrap();
    db.insert_task(
        project,
        "Existing",
        "unchanged",
        "developer",
        TaskPriority::Normal,
    )
    .unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();
    let before = app.tasks().unwrap();
    for kind in [
        LeadDecisionKind::DirectTasks,
        LeadDecisionKind::PlanRequired,
    ] {
        app.invoke_lead(
            "assess",
            &FakeLead {
                contexts: RefCell::new(Vec::new()),
                response: RefCell::new(Some(LeadBackendResponse {
                    message: "assessment".into(),
                    proposals: Vec::new(),
                    decision: Some(LeadDecision {
                        kind,
                        details: serde_json::json!({"next": "operator"}),
                    }),
                })),
            },
            20,
        )
        .unwrap();
    }
    let decisions = app.lead_decisions().unwrap();
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].status, "superseded");
    assert!(!decisions[0].actionable);
    assert_eq!(decisions[1].status, "pending");
    assert!(decisions[1].actionable);
    assert_eq!(app.tasks().unwrap(), before);
}

#[test]
fn malformed_decision_output_fails_without_state_mutation_or_planner_call() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("malformed-decision.sqlite");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("decision project").unwrap();
    db.insert_task(
        project,
        "Existing",
        "unchanged",
        "developer",
        TaskPriority::Normal,
    )
    .unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();
    let before = app.tasks().unwrap();
    assert!(app.invoke_lead("assess", &InvalidDecisionLead, 20).is_err());
    assert_eq!(app.tasks().unwrap(), before);
    assert!(app.pending_lead_decision().unwrap().is_none());
    assert!(app.lead().context(20).unwrap().proposals.is_empty());
}

#[test]
fn automated_lead_invokes_only_lead_and_never_planner_or_task_creation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("automated-lead-read-only.sqlite");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("decision project").unwrap();
    db.insert_agent(&AgentDefinition {
        actions: vec![orc::registry::AgentAction::Lead],
        ..codex_agent("/profiles/lead")
    })
    .unwrap();
    db.insert_task(
        project,
        "Existing",
        "unchanged",
        "developer",
        TaskPriority::Normal,
    )
    .unwrap();
    drop(db);

    struct Spy {
        calls: RefCell<Vec<orc::registry::AgentAction>>,
    }
    impl ActionBackend for Spy {
        fn invoke(
            &self,
            _agent: &AgentDefinition,
            action: orc::registry::AgentAction,
            _input: &str,
            _model: Option<&str>,
            _effort: Option<ReasoningEffort>,
        ) -> anyhow::Result<ActionExecution> {
            self.calls.borrow_mut().push(action);
            Ok(ActionExecution {
                output: r#"{"message":"assess","proposals":[],"decision":{"kind":"PLAN_REQUIRED","details":{"next":"operator"}}}"#.into(),
                token_usage: None,
            })
        }
    }

    let app = OrcApp::open(&path, dir.path()).unwrap();
    let before = app.tasks().unwrap();
    let spy = Spy {
        calls: RefCell::new(Vec::new()),
    };
    app.automated_lead_with_backend("assess", &ActionOverrides::default(), &spy)
        .unwrap();
    assert_eq!(
        spy.calls.into_inner(),
        vec![orc::registry::AgentAction::Lead]
    );
    assert_eq!(app.tasks().unwrap(), before);
    assert!(app.pending_lead_decision().unwrap().is_some());
}

#[test]
fn configured_lead_decisions_are_canonical_read_only_and_restart_safe() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("configured-lead.sqlite");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("configured lead").unwrap();
    db.insert_task(
        project,
        "Existing",
        "unchanged",
        "developer",
        TaskPriority::Normal,
    )
    .unwrap();
    let mut lead_agent = codex_agent("/profiles/lead");
    lead_agent.actions = vec![orc::registry::AgentAction::Lead];
    db.insert_agent(&lead_agent).unwrap();
    db.set_lead_provider_config(&LeadProviderConfig {
        agent_id: lead_agent.id.clone(),
        model: Some("configured-model".into()),
        reasoning_effort: Some(ReasoningEffort::Medium),
    })
    .unwrap();
    drop(db);

    struct CanonicalLead {
        kind: LeadDecisionKind,
    }
    impl ActionBackend for CanonicalLead {
        fn invoke(
            &self,
            _: &AgentDefinition,
            action: orc::registry::AgentAction,
            _: &str,
            _: Option<&str>,
            _: Option<ReasoningEffort>,
        ) -> anyhow::Result<ActionExecution> {
            assert_eq!(action, orc::registry::AgentAction::Lead);
            let details = if self.kind == LeadDecisionKind::DirectTasks {
                serde_json::json!({"tasks": [planned_task("Canonical") ]})
            } else {
                serde_json::json!({"operator": "next"})
            };
            Ok(ActionExecution {
                output: serde_json::json!({
                    "message": "assessment",
                    "proposals": [],
                    "decision": {"kind": self.kind, "details": details}
                })
                .to_string(),
                token_usage: None,
            })
        }
    }

    for kind in [
        LeadDecisionKind::DirectTasks,
        LeadDecisionKind::PlanRequired,
        LeadDecisionKind::UserDecisionRequired,
    ] {
        let app = OrcApp::open(&path, dir.path()).unwrap();
        let before = app.tasks().unwrap();
        let (run_id, response) = app
            .automated_lead_with_backend(
                "assess",
                &ActionOverrides::default(),
                &CanonicalLead { kind },
            )
            .unwrap();
        assert_eq!(response.decision.unwrap().kind, kind);
        assert_eq!(app.tasks().unwrap(), before);
        let persisted = app.pending_lead_decision().unwrap().unwrap();
        assert_eq!(persisted.run_id, Some(run_id));
        if kind == LeadDecisionKind::DirectTasks {
            let details: serde_json::Value = serde_json::from_str(&persisted.details).unwrap();
            let tasks = details
                .get("tasks")
                .and_then(serde_json::Value::as_array)
                .expect("DIRECT_TASKS details persist canonical tasks");
            assert_eq!(tasks.len(), 1);
            assert_eq!(
                tasks[0].get("local_id").and_then(|v| v.as_str()),
                Some("lead-task")
            );
            assert_eq!(
                tasks[0].get("title").and_then(|v| v.as_str()),
                Some("Canonical")
            );
        }
        drop(app);
        let reopened = OrcApp::open(&path, dir.path()).unwrap();
        let reopened_decision = reopened.pending_lead_decision().unwrap().unwrap();
        assert_eq!(reopened_decision.run_id, Some(run_id));
        if kind == LeadDecisionKind::DirectTasks {
            let details: serde_json::Value =
                serde_json::from_str(&reopened_decision.details).unwrap();
            assert_eq!(details["tasks"][0]["title"], "Canonical");
        }
        reopened.consume_pending_lead_decision().unwrap();
    }
}
#[test]
fn applying_and_rejecting_are_explicit_and_project_scoped() {
    let dir = tempdir().unwrap();
    let first_path = dir.path().join("first.db");
    let second_path = dir.path().join("second.db");
    for (path, name) in [(&first_path, "first"), (&second_path, "second")] {
        let db = Database::init(path).unwrap();
        db.create_project(name).unwrap();
    }
    let first = OrcApp::open(&first_path, dir.path()).unwrap();
    let backend = FakeLead {
        contexts: RefCell::new(Vec::new()),
        response: RefCell::new(Some(LeadBackendResponse {
            message: "proposal".into(),
            proposals: vec![
                LeadProposalKind::Task(planned_task("Applied")),
                LeadProposalKind::Task(planned_task("Rejected")),
            ],
            decision: None,
        })),
    };
    let response = first.invoke_lead("plan", &backend, 10).unwrap();
    let second = OrcApp::open(&second_path, dir.path()).unwrap();
    assert!(
        second
            .apply_lead_proposal(response.proposals[0].id)
            .is_err()
    );
    first.apply_lead_proposal(response.proposals[0].id).unwrap();
    assert!(
        first
            .lead()
            .reject_proposal(response.proposals[1].id)
            .unwrap()
    );
    assert_eq!(first.tasks().unwrap().len(), 1);
    assert!(second.tasks().unwrap().is_empty());
    assert!(first.lead().pending_proposals().unwrap().is_empty());
}

#[test]
fn proposal_application_is_single_use_and_ends_applied() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.create_project("alpha").unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();
    let backend = FakeLead {
        contexts: RefCell::new(Vec::new()),
        response: RefCell::new(Some(LeadBackendResponse {
            message: "proposal".into(),
            proposals: vec![LeadProposalKind::Task(planned_task("Once"))],
            decision: None,
        })),
    };
    let id = app.invoke_lead("plan", &backend, 10).unwrap().proposals[0].id;

    app.apply_lead_proposal(id).unwrap();
    assert!(app.apply_lead_proposal(id).is_err());
    assert_eq!(app.tasks().unwrap().len(), 1);
    assert_eq!(
        app.lead().context(10).unwrap().proposals[0].status,
        LeadProposalStatus::Applied
    );
}

#[test]
fn claim_excludes_other_applications_and_rejection() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("alpha").unwrap();
    let id = db
        .record_lead_proposal(project, &LeadProposalKind::Task(planned_task("Claimed")))
        .unwrap();
    assert!(
        db.transition_lead_proposal(
            project,
            id,
            LeadProposalStatus::Pending,
            LeadProposalStatus::Applying,
        )
        .unwrap()
    );
    let second = Database::open(&path).unwrap();
    assert!(
        !second
            .transition_lead_proposal(
                project,
                id,
                LeadProposalStatus::Pending,
                LeadProposalStatus::Applying,
            )
            .unwrap()
    );
    let app = OrcApp::open(&path, dir.path()).unwrap();
    assert!(!app.lead().reject_proposal(id).unwrap());
    assert!(app.apply_lead_proposal(id).is_err());
    assert_eq!(
        second
            .get_lead_proposal(project, id)
            .unwrap()
            .unwrap()
            .status,
        LeadProposalStatus::Applying
    );
}

#[test]
fn rejected_proposal_cannot_be_applied() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("alpha").unwrap();
    let id = db
        .record_lead_proposal(project, &LeadProposalKind::Task(planned_task("Rejected")))
        .unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();
    assert!(app.lead().reject_proposal(id).unwrap());
    assert!(app.apply_lead_proposal(id).is_err());
    assert!(app.tasks().unwrap().is_empty());
}

#[test]
fn failed_application_returns_proposal_to_pending() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("alpha").unwrap();
    let mut task = planned_task("Invalid dependency");
    task.depends_on.push("missing".into());
    let id = db
        .record_lead_proposal(project, &LeadProposalKind::Task(task))
        .unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();

    assert!(app.apply_lead_proposal(id).is_err());
    assert_eq!(
        app.lead().pending_proposals().unwrap()[0].status,
        LeadProposalStatus::Pending
    );
    assert!(app.tasks().unwrap().is_empty());
}

#[test]
fn failed_invocation_preserves_user_turn_and_records_system_failure() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.create_project("alpha").unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();

    assert!(app.invoke_lead("help", &FailingLead, 10).is_err());
    let turns = app.lead().context(10).unwrap().turns;
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].role, LeadRole::User);
    assert_eq!(turns[1].role, LeadRole::System);
    assert!(turns[1].content.contains("provider unavailable"));
}

#[test]
fn production_lead_backend_uses_configured_codex_agent_and_overrides() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.create_project("alpha").unwrap();
    db.insert_agent(&codex_agent("/profiles/lead")).unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();
    let config = LeadProviderConfig {
        agent_id: "project-lead".into(),
        model: Some("override-model".into()),
        reasoning_effort: Some(ReasoningEffort::High),
    };
    let agent = app.agents().unwrap().remove(0);
    let backend =
        CodexLeadBackend::from_agent(&agent, dir.path(), config.model, config.reasoning_effort)
            .unwrap();
    let args = backend.command_args("prompt");
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "override-model"])
    );
    assert!(args.iter().any(|arg| arg == "read-only"));
    assert!(!args.iter().any(|arg| arg == "workspace-write"));
    assert!(args.iter().any(|arg| arg.contains("high")));
}

#[test]
fn malformed_provider_output_creates_no_proposals_and_persists_failure_history() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.create_project("alpha").unwrap();
    drop(db);
    let app = OrcApp::open(&path, dir.path()).unwrap();

    assert!(app.invoke_lead("help", &MalformedProviderLead, 10).is_err());
    let context = app.lead().context(10).unwrap();
    assert!(context.proposals.is_empty());
    assert_eq!(context.turns.len(), 2);
    assert_eq!(context.turns[0].role, LeadRole::User);
    assert_eq!(context.turns[1].role, LeadRole::System);
    assert!(
        context.turns[1]
            .content
            .contains("malformed structured output")
    );
}

#[test]
fn applying_proposal_survives_reopen_and_requires_explicit_recovery() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("alpha").unwrap();
    let applying = db
        .record_lead_proposal(project, &LeadProposalKind::Task(planned_task("Applying")))
        .unwrap();
    let applied = db
        .record_lead_proposal(project, &LeadProposalKind::Task(planned_task("Applied")))
        .unwrap();
    let rejected = db
        .record_lead_proposal(project, &LeadProposalKind::Task(planned_task("Rejected")))
        .unwrap();
    assert!(
        db.transition_lead_proposal(
            project,
            applying,
            LeadProposalStatus::Pending,
            LeadProposalStatus::Applying
        )
        .unwrap()
    );
    assert!(
        db.transition_lead_proposal(
            project,
            applied,
            LeadProposalStatus::Pending,
            LeadProposalStatus::Applying
        )
        .unwrap()
    );
    assert!(
        db.transition_lead_proposal(
            project,
            applied,
            LeadProposalStatus::Applying,
            LeadProposalStatus::Applied
        )
        .unwrap()
    );
    assert!(
        db.resolve_lead_proposal(project, rejected, LeadProposalStatus::Rejected)
            .unwrap()
    );
    drop(db);

    let app = OrcApp::open(&path, dir.path()).unwrap();
    let proposal = app
        .lead()
        .context(10)
        .unwrap()
        .proposals
        .into_iter()
        .find(|item| item.id == applying)
        .unwrap();
    assert_eq!(proposal.status, LeadProposalStatus::Applying);
    assert!(proposal.applying_at.is_some());
    app.recover_lead_proposal(applying).unwrap();
    let proposal = app
        .lead()
        .context(10)
        .unwrap()
        .proposals
        .into_iter()
        .find(|item| item.id == applying)
        .unwrap();
    assert_eq!(proposal.status, LeadProposalStatus::Pending);
    assert!(proposal.applying_at.is_none());
    assert!(app.recover_lead_proposal(applying).is_err());
    assert!(app.recover_lead_proposal(applied).is_err());
    assert!(app.recover_lead_proposal(rejected).is_err());
}
