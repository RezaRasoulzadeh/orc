use orc::app::OrcApp;
use orc::lead::LeadDecisionKind;
use orc::protocol::{ExecutionHints, TaskProposal};
use orc::storage::Database;
use orc::storage::db::LeadDecisionMetadata;
use orc::task::{TaskPriority, TaskScopeMode};
use std::process::Command;
use tempfile::tempdir;

fn proposal(local_id: &str, depends_on: Vec<&str>) -> TaskProposal {
    TaskProposal {
        local_id: local_id.into(),
        title: format!("{local_id} title"),
        objective: format!("{local_id} objective"),
        role: "developer".into(),
        priority: TaskPriority::High,
        depends_on: depends_on.into_iter().map(str::to_owned).collect(),
        capabilities: vec!["rust".into()],
        scope_mode: Some(TaskScopeMode::Module),
        context_files: vec!["src/lib.rs".into()],
        expected_changes: vec!["src/lib.rs".into()],
        unchanged: vec!["docs".into()],
        acceptance_criteria: vec!["works".into()],
        required_tests: vec!["integration test".into()],
        validation: vec!["cargo test".into()],
        execution_hints: ExecutionHints {
            class: Some("focused".into()),
            model: Some("m".into()),
            effort: Some("high".into()),
        },
    }
}

fn fixture() -> (tempfile::TempDir, Database, i64) {
    let dir = tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".orc")).unwrap();
    let db = Database::init(dir.path().join(".orc/orc.db")).unwrap();
    let project = db.create_project("apply").unwrap();
    (dir, db, project)
}

fn persist(db: &Database, project: i64, kind: LeadDecisionKind, tasks: serde_json::Value) {
    db.record_lead_decision(
        project,
        &kind,
        &serde_json::json!({"tasks": tasks}),
        LeadDecisionMetadata {
            snapshot: "snapshot",
            run_id: None,
            source_request: "apply test request",
            summary: "apply test decision",
        },
    )
    .unwrap();
}

#[test]
fn applies_single_task_with_exact_canonical_fields_and_consumes_once() {
    let (dir, db, project) = fixture();
    let task = proposal("one", vec![]);
    persist(
        &db,
        project,
        LeadDecisionKind::DirectTasks,
        serde_json::to_value(vec![&task]).unwrap(),
    );
    let app = OrcApp::open(dir.path().join(".orc/orc.db"), dir.path()).unwrap();
    let mapping = app.apply_pending_lead_decision().unwrap().unwrap();
    assert_eq!(mapping["one"], "T-0001");
    let created = db.list_tasks().unwrap().pop().unwrap();
    assert_eq!(
        (
            created.title,
            created.objective,
            created.role,
            created.priority
        ),
        (
            task.title.clone(),
            task.objective.clone(),
            task.role.clone(),
            TaskPriority::High
        )
    );
    assert_eq!(created.required_capabilities, task.capabilities);
    assert_eq!(created.scope_mode, task.scope_mode);
    assert_eq!(created.context_files, task.context_files);
    assert_eq!(created.expected_changes, task.expected_changes);
    let metadata = db.get_task_proposal_metadata(&created.id).unwrap().unwrap();
    assert_eq!(metadata.acceptance_criteria, task.acceptance_criteria);
    assert_eq!(metadata.required_tests, task.required_tests);
    assert_eq!(metadata.validation, task.validation);
    assert_eq!(metadata.execution_hints, task.execution_hints);
    assert!(app.apply_pending_lead_decision().unwrap().is_none());
    assert!(!app.lead_decisions().unwrap()[0].actionable);
}

#[test]
fn applies_dependencies_and_preserves_history_after_restart() {
    let (dir, db, project) = fixture();
    let a = proposal("a", vec![]);
    let b = proposal("b", vec!["a"]);
    persist(
        &db,
        project,
        LeadDecisionKind::DirectTasks,
        serde_json::to_value(vec![a, b]).unwrap(),
    );
    let app = OrcApp::open(dir.path().join(".orc/orc.db"), dir.path()).unwrap();
    let map = app.apply_pending_lead_decision().unwrap().unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(
        db.list_task_dependencies(&map["b"]).unwrap(),
        vec![map["a"].clone()]
    );
    drop(app);
    drop(db);
    let reopened = Database::open(dir.path().join(".orc/orc.db")).unwrap();
    let persisted_tasks = reopened.list_tasks().unwrap();
    assert_eq!(persisted_tasks.len(), 2);
    assert_eq!(persisted_tasks[0].id, map["a"]);
    assert_eq!(persisted_tasks[1].id, map["b"]);
    assert_eq!(reopened.list_lead_decisions(project).unwrap().len(), 1);
    assert!(!reopened.list_lead_decisions(project).unwrap()[0].actionable);
}

#[test]
fn rejects_wrong_kind_and_malformed_or_invalid_proposals_atomically() {
    let (dir, db, project) = fixture();
    persist(
        &db,
        project,
        LeadDecisionKind::PlanRequired,
        serde_json::json!([]),
    );
    let app = OrcApp::open(dir.path().join(".orc/orc.db"), dir.path()).unwrap();
    assert!(app.apply_pending_lead_decision().is_err());
    assert!(db.list_tasks().unwrap().is_empty());
    drop(app);
    persist(
        &db,
        project,
        LeadDecisionKind::DirectTasks,
        serde_json::json!([{"local_id":"bad","title":"","objective":"x","role":"developer","priority":"normal","depends_on":[],"capabilities":[],"scope_mode":null,"context_files":[],"expected_changes":["x"],"unchanged":["x"],"acceptance_criteria":["x"],"required_tests":["x"],"validation":["x"],"execution_hints":{}}]),
    );
    let app = OrcApp::open(dir.path().join(".orc/orc.db"), dir.path()).unwrap();
    assert!(app.apply_pending_lead_decision().is_err());
    assert!(db.list_tasks().unwrap().is_empty());
    assert!(app.pending_lead_decision().unwrap().unwrap().actionable);
}

#[test]
fn cli_apply_creates_tasks_without_dispatching_them() {
    let (dir, db, project) = fixture();
    persist(
        &db,
        project,
        LeadDecisionKind::DirectTasks,
        serde_json::to_value(vec![proposal("cli", vec![])]).unwrap(),
    );
    let output = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(dir.path())
        .args(["lead", "apply"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tasks = db.list_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, orc::task::TaskStatus::Backlog);
    assert!(db.list_agent_runs(project, 100).unwrap().is_empty());
}
