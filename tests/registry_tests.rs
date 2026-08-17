use orc::backend::WorkerFactory;
use orc::registry::{self, AgentDefinition};
use orc::storage::Database;
use orc::task::TaskPriority;
use orc::worker::{AntigravityWorker, CodexWorker};
use tempfile::tempdir;

fn agent(id: &str, priority: i64, status: &str) -> AgentDefinition {
    AgentDefinition {
        id: id.into(),
        backend: "codex".into(),
        execution_mode: "automated".into(),
        display_name: id.into(),
        enabled: true,
        priority,
        capabilities: vec!["code".into(), "terminal".into()],
        status: status.into(),
        unavailable_reason: None,
        profile_path: Some(format!("/profiles/{id}")),
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
    }
}

#[test]
fn registry_persists_multiple_profiles_and_reopens() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.insert_agent(&agent("codex-main", 100, registry::AVAILABLE))
        .unwrap();
    db.insert_agent(&agent("codex-secondary", 90, registry::AVAILABLE))
        .unwrap();
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let agents = reopened.list_agents().unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].backend, "codex");
    assert_eq!(agents[0].execution_mode, "automated");
    assert_eq!(
        agents[1].profile_path.as_deref(),
        Some("/profiles/codex-secondary")
    );
}

#[test]
fn manual_execution_mode_persists() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let mut manual = agent("chatgpt", 100, registry::AVAILABLE);
    manual.backend = "chatgpt".into();
    manual.execution_mode = registry::MANUAL.into();
    db.insert_agent(&manual).unwrap();
    assert_eq!(
        db.get_agent("chatgpt").unwrap().unwrap().execution_mode,
        registry::MANUAL
    );
}

#[test]
fn selection_filters_and_orders_deterministically() {
    let agents = vec![
        agent("codex-z", 100, registry::AVAILABLE),
        agent("codex-a", 100, registry::AVAILABLE),
        agent("disabled", 200, registry::AVAILABLE),
        agent("unavailable", 300, registry::UNAVAILABLE),
    ];
    let mut agents = agents;
    agents[2].enabled = false;
    let required = vec!["code".into(), "terminal".into()];
    assert_eq!(
        registry::select_agent(&agents, &required).unwrap().id,
        "codex-a"
    );
    agents[0].capabilities = vec!["code".into()];
    assert_eq!(
        registry::select_agent(&agents, &required).unwrap().id,
        "codex-a"
    );
}

#[test]
fn enablement_and_availability_are_persisted() {
    let dir = tempdir().unwrap();
    let db = Database::init(dir.path().join("orc.db")).unwrap();
    db.insert_agent(&agent("codex-main", 100, registry::AVAILABLE))
        .unwrap();
    assert!(db.set_agent_enabled("codex-main", false).unwrap());
    assert!(
        db.set_agent_availability("codex-main", registry::UNAVAILABLE, Some("quota"))
            .unwrap()
    );
    let saved = db.get_agent("codex-main").unwrap().unwrap();
    assert!(!saved.enabled);
    assert_eq!(saved.status, registry::UNAVAILABLE);
    assert_eq!(saved.unavailable_reason.as_deref(), Some("quota"));
}

#[test]
fn codex_workers_and_factory_support_isolated_profiles() {
    let first = agent("codex-main", 100, registry::AVAILABLE);
    let second = agent("codex-secondary", 90, registry::AVAILABLE);
    let first_worker = WorkerFactory::build(&first).unwrap();
    let second_worker = WorkerFactory::build(&second).unwrap();
    assert_eq!(
        CodexWorker::command_args("inspect"),
        vec!["exec", "--sandbox", "workspace-write", "inspect"]
    );
    assert_ne!(first.profile_path, second.profile_path);
    drop((first_worker, second_worker));
}

#[test]
fn antigravity_worker_is_built_in_print_json_accept_edits_mode() {
    let mut definition = agent("antigravity-main", 100, registry::AVAILABLE);
    definition.backend = "antigravity".into();
    let worker = WorkerFactory::build(&definition).unwrap();
    assert_eq!(
        AntigravityWorker::command_args("inspect"),
        vec![
            "-p",
            "inspect",
            "--output-format",
            "json",
            "--mode",
            "accept-edits",
            "--sandbox",
        ]
    );
    drop(worker);
}

#[test]
fn unsupported_backend_is_rejected_by_validation_and_factory() {
    assert!(registry::validate_backend("antigravity").is_ok());
    assert!(registry::validate_backend("nonexistent").is_err());
    let mut definition = agent("bad", 100, registry::AVAILABLE);
    definition.backend = "nonexistent".into();
    assert!(WorkerFactory::build(&definition).is_err());
}

#[test]
fn agent_run_history_keeps_selected_ids_distinct() {
    let dir = tempdir().unwrap();
    let db = Database::init(dir.path().join("orc.db")).unwrap();
    let project = db.create_project("project").unwrap();
    db.insert_task(project, "First", "First task", "dev", TaskPriority::Normal)
        .unwrap();
    db.insert_task(
        project,
        "Second",
        "Second task",
        "dev",
        TaskPriority::Normal,
    )
    .unwrap();
    let first = db
        .create_agent_run(project, "T-0001", "codex-main")
        .unwrap();
    let second = db
        .create_agent_run(project, "T-0002", "codex-secondary")
        .unwrap();
    let runs = db.list_agent_runs(project, 10).unwrap();
    assert!(
        runs.iter()
            .all(|run| run.execution_mode == registry::AUTOMATED)
    );
    let ids: Vec<_> = runs
        .iter()
        .filter(|run| run.id == first || run.id == second)
        .map(|run| run.agent.as_str())
        .collect();
    assert!(ids.contains(&"codex-main"));
    assert!(ids.contains(&"codex-secondary"));
}

#[test]
fn priority_update_persists_and_changes_selection() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.insert_agent(&agent("first", 100, registry::AVAILABLE))
        .unwrap();
    db.insert_agent(&agent("second", 90, registry::AVAILABLE))
        .unwrap();
    let required = vec!["code".into()];
    assert_eq!(
        registry::select_agent(&db.list_agents().unwrap(), &required)
            .unwrap()
            .id,
        "first"
    );
    assert!(db.set_agent_priority("second", 110).unwrap());
    assert!(!db.set_agent_priority("missing", 200).unwrap());
    assert_eq!(
        registry::select_agent(&db.list_agents().unwrap(), &required)
            .unwrap()
            .id,
        "second"
    );
    drop(db);
    assert_eq!(
        Database::open(&path)
            .unwrap()
            .get_agent("second")
            .unwrap()
            .unwrap()
            .priority,
        110
    );
}

#[test]
fn quota_metadata_persists_accepts_boundaries_and_clears() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.insert_agent(&agent("codex-main", 100, registry::AVAILABLE))
        .unwrap();
    assert!(db.set_agent_quota("codex-main", -1, None).is_err());
    assert!(db.set_agent_quota("codex-main", 101, None).is_err());
    assert!(
        db.set_agent_quota("codex-main", 0, Some("2026-08-22T20:09:00+03:30"))
            .unwrap()
    );
    let saved = db.get_agent("codex-main").unwrap().unwrap();
    assert_eq!(saved.quota_remaining_percent, Some(0));
    assert_eq!(
        saved.quota_reset_at.as_deref(),
        Some("2026-08-22T20:09:00+03:30")
    );
    assert!(saved.quota_checked_at.is_some());
    assert_eq!(saved.quota_source.as_deref(), Some("manual"));
    assert_eq!(saved.status, registry::AVAILABLE);
    assert!(db.set_agent_quota("codex-main", 100, None).unwrap());
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        reopened
            .get_agent("codex-main")
            .unwrap()
            .unwrap()
            .quota_remaining_percent,
        Some(100)
    );
    assert!(reopened.clear_agent_quota("codex-main").unwrap());
    let cleared = reopened.get_agent("codex-main").unwrap().unwrap();
    assert_eq!(cleared.quota_remaining_percent, None);
    assert_eq!(cleared.quota_checked_at, None);
}
