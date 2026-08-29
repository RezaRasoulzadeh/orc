use orc::backend::{WorkerFactory, provider_adapter};
use orc::registry::{
    self, Agent, AgentCapability, AgentDefinition, AgentExecutionMode, AgentLifecycleState,
    ReasoningEffort,
};
use orc::storage::Database;
use tempfile::tempdir;

fn definition(backend: &str) -> AgentDefinition {
    let mut definition: AgentDefinition = serde_json::from_value(serde_json::json!({
        "id": "shared-coder",
        "backend": backend,
        "execution_mode": "automated",
        "display_name": "Shared Coder",
        "enabled": true,
        "priority": 10,
        "capabilities": ["coding", "terminal", "structured-output"],
        "status": "available",
        "unavailable_reason": null,
        "profile_path": "/profiles/shared-coder",
        "model": "provider-model",
        "reasoning_effort": null,
        "config_metadata": null,
        "quota_remaining_percent": null,
        "quota_reset_at": null,
        "quota_checked_at": null,
        "quota_source": null,
        "quota_limits": null,
        "actions": ["Code"]
    }))
    .unwrap();
    definition.reasoning_effort = Some(ReasoningEffort::High);
    definition
}

#[test]
fn canonical_agent_normalizes_provider_neutral_contract() {
    let agent = Agent::from_definition(&definition("codex")).unwrap();

    assert_eq!(agent.model_version, registry::AGENT_MODEL_VERSION);
    assert!(agent.is_global());
    assert_eq!(agent.execution_mode(), AgentExecutionMode::Automated);
    assert_eq!(agent.provider(), "codex");
    assert!(agent.capabilities.contains(&AgentCapability::Code));
    assert!(
        agent
            .capabilities
            .contains(&AgentCapability::CommandExecution)
    );
    assert!(
        agent
            .capabilities
            .contains(&AgentCapability::StructuredOutput)
    );
}

#[test]
fn global_agent_round_trips_through_storage_without_project_ownership() {
    let directory = tempdir().unwrap();
    let db = Database::init(directory.path().join("orc.db")).unwrap();
    let agent = Agent::from_definition(&definition("copilot")).unwrap();

    db.insert_global_agent(&agent).unwrap();
    let saved = db.get_global_agent(&agent.id).unwrap().unwrap();
    assert_eq!(saved.model_version, registry::AGENT_MODEL_VERSION);
    assert_eq!(saved.scope, registry::GLOBAL_AGENT_SCOPE);
    assert_eq!(saved.id, agent.id);
    assert_eq!(saved.roles, agent.roles);
    assert_eq!(saved.capabilities, agent.capabilities);
    assert_eq!(db.list_global_agents().unwrap().len(), 1);
}

#[test]
fn project_reference_resolves_only_explicitly_authorized_global_agents() {
    let directory = tempdir().unwrap();
    let db = Database::init(directory.path().join("orc.db")).unwrap();
    let first_project = db.create_project("first").unwrap();
    let second_project = db.create_project("second").unwrap();
    let agent = Agent::from_definition(&definition("copilot")).unwrap();
    db.insert_global_agent(&agent).unwrap();

    assert!(
        db.resolve_project_agent(first_project, &agent.id)
            .unwrap()
            .is_none()
    );
    assert!(db.reference_global_agent(first_project, &agent.id).unwrap());
    assert!(!db.reference_global_agent(first_project, &agent.id).unwrap());

    let references = db.list_project_agent_references(first_project).unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].project_id, first_project);
    assert_eq!(references[0].agent_id, agent.id);
    assert_eq!(
        db.list_project_agents(first_project).unwrap(),
        vec![agent.clone()]
    );
    assert_eq!(
        db.resolve_project_agent(first_project, &agent.id).unwrap(),
        Some(agent.clone())
    );
    assert!(
        db.resolve_project_agent(second_project, &agent.id)
            .unwrap()
            .is_none()
    );

    let reopened = Database::open(directory.path().join("orc.db")).unwrap();
    assert_eq!(
        reopened.list_project_agents(first_project).unwrap(),
        vec![agent.clone()]
    );
    assert_eq!(
        reopened
            .resolve_project_agent(first_project, &agent.id)
            .unwrap(),
        Some(agent.clone())
    );

    assert!(
        db.remove_global_agent_reference(first_project, &agent.id)
            .unwrap()
    );
    assert!(
        db.resolve_project_agent(first_project, &agent.id)
            .unwrap()
            .is_none()
    );

    db.reference_global_agent(first_project, &agent.id).unwrap();
    let task = db
        .insert_task(
            first_project,
            "Active ownership",
            "Keep the reference",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let run = db
        .create_agent_run(first_project, &task, &agent.id)
        .unwrap();
    let detach_error = db
        .remove_global_agent_reference(first_project, &agent.id)
        .unwrap_err();
    assert!(detach_error.to_string().contains("active run"));
    db.update_agent_run_status(run, "completed", None).unwrap();
    assert!(
        db.remove_global_agent_reference(first_project, &agent.id)
            .unwrap()
    );
}

#[test]
fn project_reference_rejects_non_global_agents_and_unknown_projects() {
    let directory = tempdir().unwrap();
    let db = Database::init(directory.path().join("orc.db")).unwrap();
    let project = db.create_project("project").unwrap();
    let mut project_owned = Agent::from_definition(&definition("copilot")).unwrap();
    project_owned.scope = "project".into();

    let ownership_error = db.insert_global_agent(&project_owned).unwrap_err();
    assert!(ownership_error.to_string().contains("globally owned"));
    let missing_error = db
        .reference_global_agent(project, &project_owned.id)
        .unwrap_err();
    assert!(missing_error.to_string().contains("not found"));
    let unknown_project = db
        .reference_global_agent(999, &project_owned.id)
        .unwrap_err();
    assert!(
        unknown_project
            .to_string()
            .contains("project '999' not found")
    );
}

#[test]
fn global_registry_survives_project_database_reset_without_authoritative_agent_rows() {
    let directory = tempfile::tempdir().unwrap();
    let project_db = directory.path().join("project.db");
    let registry_db = directory.path().join("global-agents.db");
    let db = Database::init_with_registry(&project_db, &registry_db).unwrap();
    let project = db.create_project("first").unwrap();
    let agent = Agent::from_definition(&definition("shared-after-reset")).unwrap();
    db.insert_global_agent(&agent).unwrap();
    db.reference_global_agent(project, &agent.id).unwrap();
    drop(db);

    std::fs::remove_file(&project_db).unwrap();
    let reopened = Database::init_with_registry(&project_db, &registry_db).unwrap();
    let replacement = reopened.create_project("replacement").unwrap();
    assert_eq!(
        reopened.get_global_agent(&agent.id).unwrap(),
        Some(agent.clone())
    );
    assert!(
        reopened
            .reference_global_agent(replacement, &agent.id)
            .unwrap()
    );
    assert_eq!(
        reopened.list_project_agents(replacement).unwrap(),
        vec![agent]
    );

    let project_only = rusqlite::Connection::open(&project_db).unwrap();
    let local_agent_rows: i64 = project_only
        .query_row("SELECT COUNT(*) FROM agents", [], |row| row.get(0))
        .unwrap();
    assert_eq!(local_agent_rows, 0, "project DB must not own global agents");
}

#[test]
fn one_time_legacy_migration_cannot_resurrect_a_purged_global_agent() {
    let directory = tempfile::tempdir().unwrap();
    let project_db = directory.path().join("project.db");
    let registry_db = directory.path().join("global-agents.db");
    let db = Database::init_with_registry(&project_db, &registry_db).unwrap();
    db.create_project("legacy").unwrap();
    drop(db);

    let project_only = rusqlite::Connection::open(&project_db).unwrap();
    project_only
        .execute("DELETE FROM meta WHERE key='agent_registry_migrated'", [])
        .unwrap();
    project_only
        .execute(
            "INSERT INTO agents(id, backend, display_name, capabilities) VALUES (?1, 'codex', 'Legacy', '[]')",
            ["legacy-global"],
        )
        .unwrap();
    drop(project_only);

    let migrated = Database::open_with_registry(&project_db, &registry_db).unwrap();
    assert!(migrated.get_agent("legacy-global").unwrap().is_some());
    migrated.purge_agent("legacy-global").unwrap();
    drop(migrated);

    let reopened = Database::open_with_registry(&project_db, &registry_db).unwrap();
    assert!(reopened.get_agent("legacy-global").unwrap().is_none());
}

#[test]
fn provider_adapter_translates_execution_but_not_lifecycle() {
    let agent = Agent::from_definition(&definition("codex")).unwrap();
    let adapter = provider_adapter(agent.provider()).unwrap();
    assert_eq!(adapter.provider_id(), "codex");

    let worker = adapter.build_worker(&agent).unwrap();
    assert_eq!(
        worker.execution_configuration(),
        (Some("provider-model"), Some(ReasoningEffort::High))
    );

    assert!(AgentLifecycleState::Available.can_transition_to(AgentLifecycleState::Unavailable));
    assert!(!AgentLifecycleState::Archived.can_transition_to(AgentLifecycleState::Available));
    assert!(
        AgentLifecycleState::Available
            .transition(AgentLifecycleState::Unavailable)
            .is_ok()
    );
}

#[test]
fn worker_factory_rejects_non_global_or_wrong_version_models() {
    let mut project_agent = Agent::from_definition(&definition("copilot")).unwrap();
    project_agent.scope = "project".into();
    let project_error = match WorkerFactory::build_global(&project_agent) {
        Ok(_) => panic!("project-owned agent was accepted"),
        Err(error) => error,
    };
    assert!(project_error.contains("not globally owned"));

    let mut future_agent = Agent::from_definition(&definition("copilot")).unwrap();
    future_agent.model_version += 1;
    let future_error = match WorkerFactory::build_global(&future_agent) {
        Ok(_) => panic!("future agent model was accepted"),
        Err(error) => error,
    };
    assert!(future_error.contains("unsupported agent model version"));
}
