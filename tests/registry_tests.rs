use orc::backend::WorkerFactory;
use orc::registry::{self, AgentDefinition, ReasoningEffort};
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
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![registry::AgentAction::Code],
    }
}

#[test]
fn manual_workspace_url_uses_metadata_then_backend_mapping() {
    let mut manual = agent("manual", 1, registry::AVAILABLE);
    manual.execution_mode = registry::MANUAL.into();
    manual.backend = "chatgpt".into();
    assert_eq!(
        registry::manual_workspace_url(&manual).unwrap().as_deref(),
        Some("https://chatgpt.com/")
    );
    manual.config_metadata =
        Some(serde_json::json!({ "manual_workspace_url": "https://example.com/work" }).to_string());
    assert_eq!(
        registry::manual_workspace_url(&manual).unwrap().as_deref(),
        Some("https://example.com/work")
    );
    manual.config_metadata = Some("not-json".into());
    assert!(registry::manual_workspace_url(&manual).is_err());
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
fn archived_agent_is_persisted_excluded_and_cannot_be_archived_twice() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.insert_agent(&agent("retired", 100, registry::AVAILABLE))
        .unwrap();

    db.archive_agent("retired").unwrap();
    let archived = db.get_agent("retired").unwrap().unwrap();
    assert_eq!(archived.status, "archived");
    assert!(!archived.enabled);
    assert!(registry::select_agent(&db.list_agents().unwrap(), &[]).is_err());
    assert!(matches!(
        db.archive_agent("retired"),
        Err(orc::storage::db::DbError::AgentAlreadyArchived(id)) if id == "retired"
    ));

    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        reopened.get_agent("retired").unwrap().unwrap().status,
        "archived"
    );
}

#[test]
fn agent_with_active_run_cannot_be_archived() {
    let dir = tempdir().unwrap();
    let db = Database::init(dir.path().join("orc.db")).unwrap();
    let project_id = db.create_project("test").unwrap();
    db.insert_agent(&agent("busy", 100, registry::AVAILABLE))
        .unwrap();
    let task_id = db
        .insert_task(
            project_id,
            "Task",
            "Objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.create_agent_run(project_id, &task_id, "busy").unwrap();

    assert!(matches!(
        db.archive_agent("busy"),
        Err(orc::storage::db::DbError::AgentHasActiveRun(id)) if id == "busy"
    ));
    assert_eq!(
        db.get_agent("busy").unwrap().unwrap().status,
        registry::AVAILABLE
    );
}

#[test]
fn codex_execution_defaults_persist_and_are_independent_per_agent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let mut first = agent("codex-main", 100, registry::AVAILABLE);
    first.model = Some("gpt-5.6-luna".into());
    first.reasoning_effort = Some(ReasoningEffort::Low);
    let mut second = agent("codex-secondary", 90, registry::AVAILABLE);
    second.model = Some("gpt-5.6-terra".into());
    second.reasoning_effort = Some(ReasoningEffort::High);
    db.insert_agent(&first).unwrap();
    db.insert_agent(&second).unwrap();
    drop(db);

    let db = Database::open(&path).unwrap();
    let first = db.get_agent("codex-main").unwrap().unwrap();
    let second = db.get_agent("codex-secondary").unwrap().unwrap();
    assert_eq!(first.model.as_deref(), Some("gpt-5.6-luna"));
    assert_eq!(first.reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(second.model.as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(second.reasoning_effort, Some(ReasoningEffort::High));
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
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.insert_agent(&agent("codex-main", 100, registry::AVAILABLE))
        .unwrap();
    db.insert_agent(&agent("codex-secondary", 90, registry::AVAILABLE))
        .unwrap();
    drop(db);
    let db = Database::open(&path).unwrap();
    let first = db.get_agent("codex-main").unwrap().unwrap();
    let second = db.get_agent("codex-secondary").unwrap().unwrap();
    let first_worker = WorkerFactory::build(&first).unwrap();
    let second_worker = WorkerFactory::build(&second).unwrap();
    assert_eq!(
        CodexWorker::command_args("inspect"),
        vec!["exec", "--json", "--sandbox", "workspace-write", "-"]
    );
    assert_eq!(
        first_worker.configured_environment().map(|(_, path)| path),
        Some(std::path::Path::new("/profiles/codex-main"))
    );
    assert_eq!(
        second_worker.configured_environment().map(|(_, path)| path),
        Some(std::path::Path::new("/profiles/codex-secondary"))
    );
}

#[test]
fn codex_worker_uses_optional_model_and_reasoning_effort_configuration() {
    assert_eq!(
        CodexWorker::command_args_with_execution(
            "inspect",
            Some("gpt-5.6-luna"),
            Some(ReasoningEffort::Low),
        ),
        vec![
            "exec",
            "--json",
            "--sandbox",
            "workspace-write",
            "--model",
            "gpt-5.6-luna",
            "--config",
            "model_reasoning_effort=\"low\"",
            "-",
        ]
    );
    assert_eq!(
        CodexWorker::command_args_with_execution("inspect", None, None),
        CodexWorker::command_args("inspect")
    );
}

#[test]
fn codex_dispatch_overrides_resolve_before_agent_defaults() {
    let mut definition = agent("codex-main", 100, registry::AVAILABLE);
    definition.model = Some("gpt-5.6-terra".into());
    definition.reasoning_effort = Some(ReasoningEffort::High);
    let worker = WorkerFactory::build_with_codex_overrides(
        &definition,
        Some("gpt-5.6-luna".into()),
        Some(ReasoningEffort::Low),
    )
    .unwrap();
    assert_eq!(
        worker.configured_environment().map(|(_, path)| path),
        Some(std::path::Path::new("/profiles/codex-main"))
    );
    assert_eq!(definition.model.as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(definition.reasoning_effort, Some(ReasoningEffort::High));
}

#[test]
fn non_codex_workers_reject_execution_overrides() {
    let mut definition = agent("copilot", 100, registry::AVAILABLE);
    definition.backend = "copilot".into();
    let error = match WorkerFactory::build_with_codex_overrides(
        &definition,
        Some("gpt-5.6-luna".into()),
        None,
    ) {
        Ok(_) => panic!("non-Codex worker unexpectedly accepted overrides"),
        Err(error) => error,
    };
    assert!(error.contains("does not support"));
}

#[test]
fn codex_worker_factory_rejects_missing_profile() {
    let mut definition = agent("codex-missing", 100, registry::AVAILABLE);
    definition.profile_path = None;
    let error = match WorkerFactory::build(&definition) {
        Ok(_) => panic!("missing Codex profile unexpectedly built a worker"),
        Err(error) => error,
    };
    assert!(error.contains("codex-missing"));
    assert!(error.contains("profile path"));
}

#[test]
fn profile_update_persists_across_reopen_and_rejects_missing_agent() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    db.insert_agent(&agent("codex-third", 100, registry::AVAILABLE))
        .unwrap();
    assert!(
        db.set_agent_profile_path("codex-third", "/profiles/third")
            .unwrap()
    );
    assert!(
        !db.set_agent_profile_path("missing-agent", "/profiles/missing")
            .unwrap()
    );
    drop(db);

    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        reopened
            .get_agent("codex-third")
            .unwrap()
            .unwrap()
            .profile_path
            .as_deref(),
        Some("/profiles/third")
    );
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
