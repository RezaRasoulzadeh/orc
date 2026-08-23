use orc::Database;
use orc::app::OrcApp;
use orc::lead::{
    CodexLeadBackend, LeadBackend, LeadBackendResponse, LeadContext, LeadProposalKind,
    LeadProposalStatus, LeadProviderConfig, LeadRole,
};
use orc::protocol::PlannedTask;
use orc::registry::{AgentDefinition, ReasoningEffort};
use orc::task::{TaskPriority, TaskScopeMode};
use std::cell::RefCell;
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
        expected_changes: Vec::new(),
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
