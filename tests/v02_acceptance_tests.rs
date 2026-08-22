use orc::app::OrcApp;
use orc::events::AppEvent;
use orc::lead::{
    LeadBackend, LeadBackendResponse, LeadContext, LeadProposalKind, LeadProposalStatus,
};
use orc::protocol::PlannedTask;
use orc::protocol::{PROTOCOL_VERSION, PlanResponse};
use orc::registry::{AVAILABLE, AgentDefinition, MANUAL};
use orc::storage::Database;
use orc::task::{TaskPriority, TaskStatus};
use std::cell::RefCell;
use std::fs;
use tempfile::tempdir;

struct ManualLead {
    response: RefCell<Option<LeadBackendResponse>>,
}

impl LeadBackend for ManualLead {
    fn invoke(&self, _: &LeadContext, _: &str) -> Result<LeadBackendResponse, String> {
        self.response
            .borrow_mut()
            .take()
            .ok_or_else(|| "response already consumed".into())
    }
}

fn task(title: &str) -> PlannedTask {
    PlannedTask {
        local_id: title.to_ascii_uppercase(),
        title: title.into(),
        objective: "Complete the acceptance scenario".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: Vec::new(),
        capabilities: Vec::new(),
        scope_mode: None,
        context_files: Vec::new(),
        expected_changes: Vec::new(),
    }
}

fn manual_agent() -> AgentDefinition {
    AgentDefinition {
        id: "manual-acceptance".into(),
        backend: "generic_manual".into(),
        execution_mode: MANUAL.into(),
        display_name: "Acceptance Manual Agent".into(),
        enabled: true,
        priority: 100,
        capabilities: vec!["code".into(), "terminal".into()],
        status: AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: Some(r#"{"manual_workspace_url":"https://manual.test/"}"#.into()),
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
    }
}

#[test]
fn v02_shared_api_covers_planning_approvals_reports_agents_and_manual_runs() {
    let directory = tempdir().unwrap();
    fs::create_dir_all(directory.path().join(".orc")).unwrap();
    fs::write(
        directory.path().join(".orc/engineering.md"),
        "# Acceptance engineering context\n\n## Tests and validation\nEvery implementation must pass cargo test.\n",
    )
    .unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    db.create_project("control plane").unwrap();
    drop(db);

    let app = OrcApp::open(&database, directory.path()).unwrap();
    let report = app.project_report().unwrap();
    assert_eq!(report.project.name, "control plane");
    assert_eq!(
        report.project.repository,
        directory.path().display().to_string()
    );
    assert!(
        report
            .engineering_contract
            .contains("Acceptance engineering context")
    );

    let request = app.planning_request().unwrap();
    request.validate().unwrap();
    assert_eq!(request.project.unwrap().name, "control plane");
    let response = PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "Create dependent acceptance tasks".into(),
        assumptions: vec![],
        risks: vec![],
        questions: vec![],
        tasks: vec![
            task("Plan root"),
            PlannedTask {
                local_id: "PLAN-CHILD".into(),
                title: "Plan child".into(),
                objective: "Depend on the root".into(),
                role: "developer".into(),
                priority: TaskPriority::Normal,
                depends_on: vec!["PLAN ROOT".into()],
                capabilities: vec![],
                scope_mode: None,
                context_files: vec![],
                expected_changes: vec![],
            },
        ],
    };
    let json = serde_json::to_string(&response).unwrap();
    let validated = app.validate_plan_json(&json).unwrap();
    let task_ids = app.apply_plan(&validated).unwrap();
    let root_id = task_ids.get("PLAN ROOT").unwrap().clone();
    let child_id = task_ids.get("PLAN-CHILD").unwrap().clone();
    assert!(app.task(&root_id).unwrap().is_some());
    let db = Database::open(&database).unwrap();
    assert_eq!(
        db.list_task_dependencies(&child_id).unwrap(),
        vec![root_id.clone()]
    );
    drop(db);

    app.configure_agent(manual_agent()).unwrap();
    assert_eq!(app.agents().unwrap()[0].id, "manual-acceptance");
    assert_eq!(
        app.agents().unwrap()[0].config_metadata.as_deref(),
        Some(r#"{"manual_workspace_url":"https://manual.test/"}"#)
    );

    let db = Database::open(&database).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    let approval_id = db
        .insert_approval_request(project, "acceptance approval")
        .unwrap();
    drop(db);
    assert!(!app.approvals().unwrap()[0].resolved);
    app.resolve_approval(approval_id).unwrap();
    assert!(app.approvals().unwrap()[0].resolved);

    let manual_task = app
        .apply_plan(&PlanResponse {
            protocol_version: PROTOCOL_VERSION,
            objective: "Manual execution".into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![task("Manual execution")],
        })
        .unwrap()
        .remove("MANUAL EXECUTION")
        .unwrap();
    let dispatch = app
        .dispatch(&manual_task, Some("manual-acceptance"))
        .unwrap();
    let run_id = dispatch.run_id;
    let manual_runs = app.manual_runs("manual-acceptance").unwrap();
    assert_eq!(manual_runs.len(), 1);
    assert_eq!(manual_runs[0].run.id, run_id);
    assert!(manual_runs[0].task_packet.contains("manual-acceptance"));
    app.submit_manual_run(run_id, "provider-independent handoff")
        .unwrap();
    assert_eq!(app.runs(10).unwrap()[0].status, "completed");

    drop(app);
    let reopened = OrcApp::open(&database, directory.path()).unwrap();
    assert!(reopened.approvals().unwrap()[0].resolved);
    let db = Database::open(&database).unwrap();
    assert_eq!(db.list_task_dependencies(&child_id).unwrap(), vec![root_id]);
    drop(db);
    assert_eq!(reopened.agents().unwrap()[0].id, "manual-acceptance");
}

#[test]
fn shared_api_and_desktop_read_models_cover_provider_independent_control_plane() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    let project = db.create_project("acceptance").unwrap();
    let task_id = db
        .insert_task(
            project,
            "Existing",
            "Inspect state",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let run_id = db
        .create_agent_run(project, &task_id, "manual-agent")
        .unwrap();
    db.update_task_status(&task_id, TaskStatus::Active).unwrap();
    drop(db);

    let app = OrcApp::open(&database, directory.path()).unwrap();
    let subscription = app.subscribe();
    let dashboard = app.dashboard(20).unwrap();
    assert_eq!(dashboard.tasks.len(), 1);
    assert_eq!(dashboard.running_agents.len(), 1);
    assert_eq!(app.runs_workspace(20, 20).unwrap().runs[0].id, run_id);
    assert_eq!(app.run_details(run_id, 20).unwrap().unwrap().run.id, run_id);

    app.requeue(&task_id).unwrap();
    assert!(matches!(
        subscription.recv().unwrap(),
        AppEvent::TaskLifecycle(_)
    ));
    assert_eq!(
        app.task(&task_id).unwrap().unwrap().status,
        TaskStatus::Backlog
    );
    assert_eq!(
        app.dashboard(20).unwrap().tasks[0].status,
        TaskStatus::Backlog
    );

    drop(app);
    let reopened = OrcApp::open(&database, directory.path()).unwrap();
    assert_eq!(
        reopened.task(&task_id).unwrap().unwrap().status,
        TaskStatus::Backlog
    );
    assert!(!reopened.lifecycle_events(20).unwrap().is_empty());
}

#[test]
fn lead_proposals_are_persisted_and_require_explicit_approval_before_mutation() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let db = Database::init(&database).unwrap();
    db.create_project("lead acceptance").unwrap();
    drop(db);

    let app = OrcApp::open(&database, directory.path()).unwrap();
    let response = app
        .invoke_lead(
            "suggest a task",
            &ManualLead {
                response: RefCell::new(Some(LeadBackendResponse {
                    message: "A task is proposed".into(),
                    proposals: vec![LeadProposalKind::Task(task("Proposed"))],
                })),
            },
            20,
        )
        .unwrap();
    assert!(app.tasks().unwrap().is_empty());
    assert_eq!(app.lead().pending_proposals().unwrap().len(), 1);
    let proposal_id = response.proposals[0].id;
    app.apply_lead_proposal(proposal_id).unwrap();
    assert_eq!(app.tasks().unwrap().len(), 1);
    assert_eq!(
        app.lead().context(20).unwrap().proposals[0].status,
        LeadProposalStatus::Applied
    );

    drop(app);
    let reopened = OrcApp::open(&database, directory.path()).unwrap();
    assert_eq!(reopened.tasks().unwrap().len(), 1);
    assert!(reopened.lead().pending_proposals().unwrap().is_empty());
}
