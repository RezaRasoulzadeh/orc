use orc::execution::ExecutionClass;
use orc::lead::LeadDecisionKind;
use orc::protocol::{ExecutionHints, PROTOCOL_VERSION, PlanResponse, TaskProposal};
use orc::registry::{AUTOMATED, EconomyTier, ReasoningEffort, ResolutionRecord};
use orc::storage::db::LeadDecisionMetadata;
use orc::storage::{AgentRunExecution, Database, WorkerResult};
use orc::task::TaskScopeMode;
use orc::task::{TaskPriority, TaskStatus};
use rusqlite::{Connection, OptionalExtension};
use tempfile::tempdir;

fn plan_response() -> PlanResponse {
    PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "ship feature".into(),
        assumptions: vec!["stable API".into()],
        risks: vec!["risk".into()],
        questions: vec!["question".into()],
        tasks: vec![
            TaskProposal {
                local_id: "first".into(),
                title: "First task".into(),
                objective: "implement first".into(),
                role: "developer".into(),
                priority: TaskPriority::High,
                depends_on: vec![],
                capabilities: vec!["rust".into()],
                scope_mode: Some(TaskScopeMode::Project),
                context_files: vec!["src/lib.rs".into()],
                expected_changes: vec!["implementation".into()],
                unchanged: vec!["other behavior".into()],
                acceptance_criteria: vec!["works".into()],
                required_tests: vec!["round trip".into()],
                validation: vec!["cargo test".into()],
                execution_hints: ExecutionHints {
                    class: Some("plan".into()),
                    model: Some("model".into()),
                    effort: Some("low".into()),
                    effort_reason: Some("isolated and well understood".into()),
                },
                risk_factors: vec![],
            },
            TaskProposal {
                local_id: "second".into(),
                title: "Second task".into(),
                objective: "implement second".into(),
                role: "reviewer".into(),
                priority: TaskPriority::Normal,
                depends_on: vec!["first".into()],
                capabilities: vec![],
                scope_mode: None,
                context_files: vec![],
                expected_changes: vec!["tests".into()],
                unchanged: vec!["first".into()],
                acceptance_criteria: vec!["passes".into()],
                required_tests: vec!["test".into()],
                validation: vec!["cargo test".into()],
                execution_hints: ExecutionHints::default(),
                risk_factors: vec![],
            },
        ],
    }
}

#[test]
fn plan_persistence_round_trip_lineage_and_atomic_provenance_validation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let repository_file = dir.path().join("repository.txt");
    std::fs::write(&repository_file, "unchanged").unwrap();
    let (project, decision, run, response, task_snapshot, repository_snapshot) = {
        let db = Database::init(&path).unwrap();
        let project = db.create_project("plan").unwrap();
        let task = db
            .insert_task(
                project,
                "existing",
                "unchanged",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let decision = db
            .record_lead_decision(
                project,
                &LeadDecisionKind::PlanRequired,
                &serde_json::json!({"plan":"needed"}),
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
        let response = plan_response();
        let task_snapshot = db.list_tasks().unwrap();
        let repository_snapshot = std::fs::read(&repository_file).unwrap();
        let id = db.store_plan(project, decision, run, &response).unwrap();
        db.store_plan(project, decision, run, &response).unwrap();
        assert_eq!(db.get_plan(id).unwrap().unwrap().response, response);
        assert_eq!(
            db.list_plan_dependencies(id).unwrap(),
            vec![("second".into(), "first".into())]
        );
        (
            project,
            decision,
            run,
            response,
            task_snapshot,
            repository_snapshot,
        )
    };
    let db = Database::open(&path).unwrap();
    let history = db.list_plan_history(project).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!((history[0].version, history[1].version), (1, 2));
    assert_eq!(
        db.get_plan(history[1].plan_id)
            .unwrap()
            .unwrap()
            .parent_plan_id,
        Some(history[0].plan_id)
    );
    assert_eq!(
        db.get_plan(history[1].plan_id).unwrap().unwrap().response,
        response
    );
    let reopened = db.get_plan(history[1].plan_id).unwrap().unwrap();
    assert_eq!(
        reopened.provenance,
        orc::storage::db::PlanProvenance::legacy(decision, run)
    );
    assert_eq!(db.list_tasks().unwrap(), task_snapshot);
    let repository_after = std::fs::read(&repository_file).unwrap();
    assert_eq!(repository_after, repository_snapshot);
    let plan_row_counts = || {
        let connection = Connection::open(&path).unwrap();
        (
            connection
                .query_row("SELECT COUNT(*) FROM plans", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            connection
                .query_row("SELECT COUNT(*) FROM plan_dependencies", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
        )
    };
    let valid_counts = plan_row_counts();
    let bad = plan_response();
    assert!(db.store_plan(project, -1, run, &bad).is_err());
    assert_eq!(db.list_plan_history(project).unwrap().len(), 2);
    assert_eq!(plan_row_counts(), valid_counts);
    let other = db.create_project("other").unwrap();
    assert!(db.store_plan(other, decision, run, &bad).is_err());
    assert_eq!(db.list_plan_history(other).unwrap().len(), 0);
    assert_eq!(plan_row_counts(), valid_counts);
    let other_decision = db
        .record_lead_decision(
            other,
            &LeadDecisionKind::PlanRequired,
            &serde_json::json!({"plan":"needed"}),
            LeadDecisionMetadata {
                snapshot: "snapshot",
                run_id: None,
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
    assert!(db.store_plan(other, other_decision, run, &bad).is_err());
    assert_eq!(plan_row_counts(), valid_counts);
    assert!(db.store_plan(project, decision, i64::MAX, &bad).is_err());
    assert_eq!(plan_row_counts(), valid_counts);
    let invalid = PlanResponse {
        tasks: vec![TaskProposal {
            depends_on: vec!["missing".into()],
            ..plan_response().tasks[0].clone()
        }],
        ..bad
    };
    assert!(db.store_plan(project, decision, run, &invalid).is_err());
    assert_eq!(db.list_plan_history(project).unwrap().len(), 2);
    assert_eq!(plan_row_counts(), valid_counts);
    let malformed = PlanResponse {
        tasks: vec![TaskProposal {
            title: String::new(),
            ..plan_response().tasks[0].clone()
        }],
        ..plan_response()
    };
    assert!(db.store_plan(project, decision, run, &malformed).is_err());
    assert_eq!(db.list_plan_history(project).unwrap().len(), 2);
    assert_eq!(plan_row_counts(), valid_counts);
}

#[test]
fn applied_plan_persists_complete_execution_contract_after_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let response = PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "persist execution contract".into(),
        assumptions: vec![],
        risks: vec![],
        questions: vec![],
        tasks: vec![TaskProposal {
            local_id: "contract".into(),
            title: "Contract task".into(),
            objective: "Preserve selected effort".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            depends_on: vec![],
            capabilities: vec![],
            scope_mode: None,
            context_files: vec![],
            expected_changes: vec!["src/lib.rs".into()],
            unchanged: vec!["unrelated behavior".into()],
            acceptance_criteria: vec!["effort survives reopen".into()],
            required_tests: vec!["storage test".into()],
            validation: vec!["cargo test".into()],
            execution_hints: ExecutionHints {
                class: Some("coder".into()),
                model: Some("task-selected-model".into()),
                effort: Some("low".into()),
                effort_reason: Some("schema and data-flow verification".into()),
            },
            risk_factors: vec![orc::protocol::TaskRiskFactor::SchemaDataFlow],
        }],
    };
    let db = Database::init(&path).unwrap();
    let project = db.create_project("contract persistence").unwrap();
    let mapping = db.apply_plan(project, &response).unwrap();
    let task_id = mapping["contract"].clone();
    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(
        db.get_task_execution_hints(&task_id).unwrap().unwrap(),
        ExecutionHints {
            class: Some("coder".into()),
            model: Some("task-selected-model".into()),
            effort: Some("low".into()),
            effort_reason: Some("schema and data-flow verification".into()),
        }
    );
    assert_eq!(
        task.effort_reason.as_deref(),
        Some("schema and data-flow verification")
    );
    assert_eq!(
        task.risk_factors,
        vec![orc::protocol::TaskRiskFactor::SchemaDataFlow]
    );
    assert_eq!(
        db.get_task_proposal_metadata(&task_id)
            .unwrap()
            .unwrap()
            .execution_hints
            .effort
            .as_deref(),
        Some("low")
    );
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let task = reopened.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(
        task.risk_policy().required_guards,
        vec![orc::protocol::TaskRiskGuard::SchemaDataFlowCoverage]
    );
    assert_eq!(
        reopened
            .get_task_execution_hints(&task_id)
            .unwrap()
            .unwrap(),
        ExecutionHints {
            class: Some("coder".into()),
            model: Some("task-selected-model".into()),
            effort: Some("low".into()),
            effort_reason: Some("schema and data-flow verification".into()),
        }
    );
    assert_eq!(
        reopened
            .get_task_proposal_metadata(&task_id)
            .unwrap()
            .unwrap()
            .execution_hints
            .effort_reason
            .as_deref(),
        Some("schema and data-flow verification")
    );
}

#[test]
fn manually_created_tasks_persist_default_execution_effort() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("default effort").unwrap();
    let task_id = db
        .insert_task(
            project,
            "Manual task",
            "Complete the manual task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();

    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_eq!(
        task.reasoning_effort,
        Some(ReasoningEffort::Low),
        "execution effort must be part of every persisted task"
    );
    assert_eq!(
        task.effort_reason.as_deref(),
        Some(orc::task::Task::DEFAULT_EFFORT_REASON)
    );

    drop(db);
    let reopened = Database::open(&path).unwrap();
    let task = reopened.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(
        task.effort_reason.as_deref(),
        Some(orc::task::Task::DEFAULT_EFFORT_REASON)
    );
}

#[test]
fn task_effort_migration_preserves_planner_selection() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("migrate effort").unwrap();
    let task_id = db
        .insert_task(
            project,
            "Planned task",
            "Preserve the selected execution depth",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let mut proposal = plan_response().tasks[0].clone();
    proposal.local_id = task_id.clone();
    proposal.execution_hints.effort = Some("high".into());
    proposal.execution_hints.effort_reason = Some("persistence and restart recovery".into());
    proposal.risk_factors = vec![orc::protocol::TaskRiskFactor::Persistence];
    db.set_task_proposal_metadata(&task_id, &proposal).unwrap();
    drop(db);

    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE tasks SET reasoning_effort = NULL, effort_reason = NULL, risk_factors = NULL WHERE id = ?1",
            [&task_id],
        )
        .unwrap();
    drop(connection);

    let reopened = Database::open(&path).unwrap();
    let task = reopened.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.reasoning_effort, Some(ReasoningEffort::High));
    assert_eq!(
        task.effort_reason.as_deref(),
        Some("persistence and restart recovery")
    );
    assert_eq!(
        task.risk_factors,
        vec![orc::protocol::TaskRiskFactor::Persistence]
    );
}

#[test]
fn agent_run_execution_survives_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let (project, task, run_id) = {
        let db = Database::init(&path).unwrap();
        let project = db.create_project("execution").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let run_id = db
            .create_agent_run_with_execution(
                project,
                &task,
                "agent",
                "automated",
                AgentRunExecution {
                    class: "coder",
                    model: Some("configured-model"),
                    effort: Some(ReasoningEffort::Low),
                    source: "template",
                },
            )
            .unwrap();
        (project, task, run_id)
    };
    let db = Database::open(&path).unwrap();
    let run = db
        .list_agent_runs(project, 10)
        .unwrap()
        .into_iter()
        .find(|run| run.id == run_id)
        .unwrap();
    assert_eq!(run.task_id.as_deref(), Some(task.as_str()));
    assert_eq!(run.execution_class, "coder");
    assert_eq!(run.resolved_model.as_deref(), Some("configured-model"));
    assert_eq!(run.resolved_reasoning_effort, Some(ReasoningEffort::Low));
    assert_eq!(run.resolution_source, "template");
}

#[test]
fn execution_template_survives_reopen_and_clear_restores_fallback() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    {
        let db = Database::init(&path).unwrap();
        db.set_execution_template(
            ExecutionClass::Coder,
            Some("persistent-model"),
            Some(ReasoningEffort::Medium),
        )
        .unwrap();
        assert_eq!(
            db.execution_template(ExecutionClass::Coder)
                .unwrap()
                .model
                .as_deref(),
            Some("persistent-model")
        );
    }
    let db = Database::open(&path).unwrap();
    let template = db.execution_template(ExecutionClass::Coder).unwrap();
    assert_eq!(template.model.as_deref(), Some("persistent-model"));
    assert_eq!(template.reasoning_effort, Some(ReasoningEffort::Medium));
    db.clear_execution_template(ExecutionClass::Coder).unwrap();
    assert_eq!(
        db.execution_template(ExecutionClass::Coder).unwrap(),
        Default::default()
    );
}

#[test]
fn opening_missing_db_fails_without_creating_file() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("missing.db");
    assert!(Database::open(&db_path).is_err());
    assert!(!db_path.exists());
}
#[test]
fn db_init_and_task_insert_read() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    // create project
    let pid = db.create_project("testproj").expect("create project");
    assert!(pid > 0);

    let tid = db
        .insert_task(pid, "T1", "Do stuff", "dev", TaskPriority::Normal)
        .expect("insert task");
    assert!(tid.starts_with("T-"));

    let tasks = db.list_tasks().expect("list");
    assert_eq!(tasks.len(), 1);
    let t = &tasks[0];
    assert_eq!(t.title, "T1");
    assert_eq!(t.status, TaskStatus::Backlog);
}

#[test]
fn reopen_preserves_data() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    {
        let db = Database::init(&db_path).expect("init");
        let pid = db.create_project("testproj").expect("create project");
        let _ = db
            .insert_task(pid, "T1", "Do stuff", "dev", TaskPriority::Normal)
            .expect("insert task");
    }

    // reopen
    let db2 = Database::open(&db_path).expect("open");
    let tasks = db2.list_tasks().expect("list");
    assert_eq!(tasks.len(), 1);
}

#[test]
fn open_migrates_legacy_schema_without_losing_project_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.db");
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP));
        CREATE TABLE agents (id TEXT PRIMARY KEY, backend TEXT NOT NULL, display_name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, priority INTEGER NOT NULL DEFAULT 0, capabilities TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'available', unavailable_reason TEXT, profile_path TEXT, config_metadata TEXT);
        CREATE TABLE tasks (id TEXT PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), title TEXT NOT NULL, objective TEXT NOT NULL, role TEXT NOT NULL, priority TEXT NOT NULL, status TEXT NOT NULL);
        CREATE TABLE approval_requests (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), reason TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP));
        CREATE TABLE agent_runs (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), task_id TEXT REFERENCES tasks(id), agent TEXT NOT NULL, status TEXT NOT NULL, output TEXT, started_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP), finished_at TEXT);
        CREATE TABLE lead_turns (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP));
        CREATE TABLE lead_decisions (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL REFERENCES projects(id), kind TEXT NOT NULL, proposal TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP));
        INSERT INTO meta VALUES ('next_task_id', '2');
        INSERT INTO projects (id, name) VALUES (7, 'legacy-project');
        INSERT INTO agents (id, backend, display_name, capabilities) VALUES ('legacy-agent', 'test', 'Legacy Agent', '[]');
        INSERT INTO tasks (id, project_id, title, objective, role, priority, status) VALUES ('T-0001', 7, 'Legacy task', 'Keep it', 'dev', 'normal', 'backlog');
        INSERT INTO approval_requests (id, project_id, reason) VALUES (11, 7, 'Legacy approval');
        INSERT INTO agent_runs (id, project_id, task_id, agent, status, output) VALUES (13, 7, 'T-0001', 'legacy-agent', 'completed', 'Legacy output');
        INSERT INTO lead_turns (id, project_id, role, content) VALUES (14, 7, 'user', 'Legacy question');
        INSERT INTO lead_decisions (id, project_id, kind, proposal) VALUES (15, 7, 'ProposeTask', 'Legacy proposal');
        ",
    )
    .unwrap();
    drop(raw);

    let db = Database::open(&path).expect("open and migrate");
    assert_eq!(
        db.get_project_name().unwrap().as_deref(),
        Some("legacy-project")
    );
    let task = db.get_task("T-0001").unwrap().unwrap();
    assert_eq!(task.title, "Legacy task");
    assert_eq!(task.scope_mode, None);
    assert!(task.context_files.is_empty());
    assert!(task.expected_changes.is_empty());
    assert_eq!(db.list_agents().unwrap().len(), 1);
    let run = &db.list_agent_runs(7, 10).unwrap()[0];
    assert_eq!(run.output.as_deref(), Some("Legacy output"));
    assert_eq!(run.execution_mode, "automated");
    assert!(!run.last_activity.is_empty());
    assert_eq!(
        db.list_approval_requests(7).unwrap()[0].reason,
        "Legacy approval"
    );
    assert!(!db.list_approval_requests(7).unwrap()[0].resolved);
    assert_eq!(
        db.list_lead_turns(7, 10).unwrap()[0].content,
        "Legacy question"
    );
    let migrated = db.list_lead_proposals(7, 10, None).unwrap();
    assert_eq!(migrated.len(), 1);
    assert!(matches!(
        migrated[0].proposal,
        orc::lead::LeadProposalKind::ApprovalRequest { .. }
    ));

    let schema = Connection::open(&path).unwrap();
    for (table, columns) in [
        ("project_facts", vec!["project_id", "key", "value"]),
        ("worker_results", vec!["run_id", "outcome", "metadata"]),
        ("lifecycle_events", vec!["id", "kind", "payload"]),
        ("worktree_metadata", vec!["agent_run_id", "branch_name"]),
    ] {
        assert!(
            schema
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(())
                )
                .is_ok()
        );
        let actual: Vec<String> = schema
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for column in columns {
            assert!(actual.iter().any(|actual_column| actual_column == column));
        }
    }
    for (table, column) in [
        ("agents", "model"),
        ("tasks", "scope_mode"),
        ("agent_runs", "phase"),
        ("approval_requests", "resolved"),
    ] {
        let exists: i64 = schema
            .query_row(
                &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                [column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }
    db.store_worktree_metadata(13, "T-0001", "legacy-branch", "/tmp/legacy-worktree")
        .unwrap();
    assert_eq!(
        db.get_worktree_metadata("T-0001").unwrap(),
        Some((
            "legacy-branch".to_string(),
            "/tmp/legacy-worktree".to_string()
        ))
    );
}

#[test]
fn repeated_init_is_idempotent_and_open_preserves_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project_id = db.create_project("project").unwrap();
    let task_id = db
        .insert_task(project_id, "Task", "Objective", "dev", TaskPriority::Normal)
        .unwrap();
    let run_id = db.create_agent_run(project_id, &task_id, "agent").unwrap();
    db.insert_approval_request(project_id, "approval").unwrap();
    drop(db);

    let db = Database::init(&path).unwrap();
    assert_eq!(db.get_project_id().unwrap(), Some(project_id));
    assert_eq!(db.list_tasks().unwrap().len(), 1);
    assert_eq!(db.list_agent_runs(project_id, 10).unwrap().len(), 1);
    assert_eq!(db.list_approval_requests(project_id).unwrap().len(), 1);
    assert_eq!(db.list_agent_runs(project_id, 10).unwrap()[0].id, run_id);
    drop(db);

    let db = Database::open(&path).unwrap();
    assert_eq!(db.get_task(&task_id).unwrap().unwrap().title, "Task");
    assert_eq!(db.list_approval_requests(project_id).unwrap().len(), 1);
}

#[test]
fn task_status_persistence() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("testproj").expect("create project");
    let tid = db
        .insert_task(pid, "T1", "Do stuff", "dev", TaskPriority::Normal)
        .expect("insert task");

    assert!(
        db.update_task_status(&tid, TaskStatus::Ready)
            .expect("update")
    );
    let t = db.get_task(&tid).expect("get task").expect("exists");
    assert_eq!(t.status, TaskStatus::Ready);
}

#[test]
fn approval_requests_are_separate_from_decisions() {
    let dir = tempdir().unwrap();
    let db = Database::init(dir.path().join("orc.db")).expect("init");
    let pid = db.create_project("testproj").expect("project");
    db.insert_approval_request(pid, "security review")
        .expect("approval");
    assert_eq!(
        db.list_approval_requests(pid).unwrap()[0].reason,
        "security review"
    );
    assert!(!db.list_approval_requests(pid).unwrap()[0].resolved);
    let id = db.list_approval_requests(pid).unwrap()[0].id;
    assert!(db.resolve_approval_request(pid, id).unwrap());
    assert!(db.list_approval_requests(pid).unwrap()[0].resolved);
}

#[test]
fn worker_result_persists_once_per_run_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let run_id;
    {
        let db = Database::init(&path).unwrap();
        let project_id = db.create_project("testproj").unwrap();
        let task_id = db
            .insert_task(project_id, "T1", "Do stuff", "dev", TaskPriority::Normal)
            .unwrap();
        run_id = db.create_agent_run(project_id, &task_id, "worker").unwrap();
        db.insert_worker_result(&WorkerResult {
            run_id,
            outcome: "timeout".into(),
            failure_category: Some("timeout".into()),
            duration_ms: Some(1000),
            metadata: Some("{\"attempt\":1}".into()),
            total_tokens: Some(130),
            input_tokens: Some(100),
            output_tokens: Some(30),
            cached_input_tokens: Some(45),
        })
        .unwrap();
        assert!(
            db.insert_worker_result(&WorkerResult {
                run_id,
                outcome: "success".into(),
                failure_category: None,
                duration_ms: None,
                metadata: None,
                total_tokens: None,
                input_tokens: None,
                output_tokens: None,
                cached_input_tokens: None,
            })
            .is_err()
        );
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.get_worker_result(run_id).unwrap(),
        Some(WorkerResult {
            run_id,
            outcome: "timeout".into(),
            failure_category: Some("timeout".into()),
            duration_ms: Some(1000),
            metadata: Some("{\"attempt\":1}".into()),
            total_tokens: Some(130),
            input_tokens: Some(100),
            output_tokens: Some(30),
            cached_input_tokens: Some(45),
        })
    );
}

fn lineage_fixture(force: bool) -> (tempfile::TempDir, Database, String, i64, String) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("purge").unwrap();
    let task = db
        .insert_task(project, "purge", "purge", "developer", TaskPriority::Normal)
        .unwrap();
    let dependency = db
        .insert_task(
            project,
            "dependency",
            "dependency",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.add_task_dependency(&task, &dependency).unwrap();
    let run_id = db.create_agent_run(project, &task, "agent").unwrap();
    db.insert_worker_result(&WorkerResult {
        run_id,
        outcome: "success".into(),
        failure_category: None,
        duration_ms: Some(100),
        metadata: Some("{\"fixture\":true}".into()),
        total_tokens: Some(10),
        input_tokens: Some(8),
        output_tokens: Some(2),
        cached_input_tokens: None,
    })
    .unwrap();
    db.update_agent_run_status(run_id, "completed", Some("done"))
        .unwrap();
    db.store_change_evidence(
        run_id,
        &orc::git::WorktreeChanges {
            files: vec![],
            stat: "evidence".into(),
            diff: String::new(),
        },
    )
    .unwrap();
    db.record_lifecycle_event("task_event", Some(&task), None, None, None)
        .unwrap();
    db.record_lifecycle_event("run_event", None, Some(run_id), None, None)
        .unwrap();
    db.store_worktree_metadata(run_id, &task, "branch", ".orc/worktrees/purge")
        .unwrap();
    db.insert_decision(project, Some(&task), "decision")
        .unwrap();
    db.update_task_status(&task, TaskStatus::Cancelled).unwrap();
    if force {
        let dependent = db
            .insert_task(
                project,
                "dependent",
                "dependent",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        db.add_task_dependency(&dependent, &task).unwrap();
        (dir, db, task, run_id, dependent)
    } else {
        (dir, db, task, run_id, dependency)
    }
}

#[test]
fn normal_task_purge_removes_complete_run_lineage_and_owned_edges() {
    let (dir, db, task, run_id, dependency) = lineage_fixture(false);
    assert!(db.get_worker_result(run_id).unwrap().is_some());
    db.purge_task(&task, false).unwrap();
    let raw = Connection::open(dir.path().join("orc.db")).unwrap();
    for (table, column, value) in [
        ("tasks", "id", task.as_str()),
        ("agent_runs", "id", &run_id.to_string()),
    ] {
        assert_eq!(
            raw.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                [value],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    for table in [
        "worker_results",
        "run_change_evidence",
        "lifecycle_events",
        "worktree_metadata",
        "decisions",
    ] {
        assert_eq!(
            raw.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM task_dependencies WHERE task_id = ?1 OR depends_on = ?1",
            [&task],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM task_dependencies WHERE depends_on = ?1",
            [&dependency],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn forced_task_purge_removes_dependent_edges_and_complete_lineage() {
    let (dir, db, task, run_id, dependent) = lineage_fixture(true);
    db.purge_task(&task, true).unwrap();
    let raw = Connection::open(dir.path().join("orc.db")).unwrap();
    assert_eq!(
        raw.query_row("SELECT COUNT(*) FROM tasks WHERE id = ?1", [&task], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM agent_runs WHERE id = ?1",
            [run_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    for table in [
        "worker_results",
        "run_change_evidence",
        "lifecycle_events",
        "worktree_metadata",
        "decisions",
    ] {
        assert_eq!(
            raw.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM task_dependencies WHERE task_id = ?1 OR depends_on = ?1",
            [&task],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        raw.query_row(
            "SELECT COUNT(*) FROM tasks WHERE id = ?1",
            [&dependent],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn dependent_task_protection_remains_without_force() {
    let (_dir, db, task, _run_id, dependent) = lineage_fixture(true);
    let error = db.purge_task(&task, false).unwrap_err().to_string();
    assert!(error.contains(&dependent));
    assert!(db.get_task(&task).unwrap().is_some());
}

#[test]
fn invalid_enum_data_is_an_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).expect("init");
    let pid = db.create_project("testproj").expect("project");
    let tid = db
        .insert_task(pid, "T1", "Do stuff", "dev", TaskPriority::Normal)
        .unwrap();
    drop(db);
    let raw = rusqlite::Connection::open(&path).unwrap();
    raw.execute("UPDATE tasks SET status = 'corrupt' WHERE id = ?1", [&tid])
        .unwrap();
    drop(raw);
    let db = Database::open(&path).unwrap();
    assert!(db.get_task(&tid).is_err());
}

#[test]
fn apply_response_is_atomic() {
    use orc::protocol::{EngineeringLeadResponse, LeadAction, PROTOCOL_VERSION};
    use rusqlite::{Connection, params};

    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");

    let db = Database::init(&path).expect("init");
    let pid = db.create_project("testproj").expect("project");

    // set next_task_id to 5 so generated ids will be T-0005, T-0006
    let raw = Connection::open(&path).unwrap();
    raw.execute("UPDATE meta SET value = '5' WHERE key = 'next_task_id'", [])
        .unwrap();

    // create a pre-existing task T-0006 so the second insertion will fail with UNIQUE constraint
    raw.execute(
        "INSERT INTO tasks (id, project_id, title, objective, role, priority, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'backlog')",
        params!["T-0006", pid, "pre", "x", "dev", "normal"],
    )
    .unwrap();
    drop(raw);

    let response = EngineeringLeadResponse {
        protocol_version: PROTOCOL_VERSION,
        message_to_cto: None,
        actions: vec![
            LeadAction::CreateTask {
                scope_mode: None,
                context_files: Vec::new(),
                expected_changes: Vec::new(),
                title: "First".into(),
                objective: "obj1".into(),
                role: "dev".into(),
                priority: orc::task::TaskPriority::Normal,
            },
            LeadAction::CreateTask {
                scope_mode: None,
                context_files: Vec::new(),
                expected_changes: Vec::new(),
                title: "Second".into(),
                objective: "obj2".into(),
                role: "dev".into(),
                priority: orc::task::TaskPriority::Normal,
            },
        ],
    };

    // applying should fail because T-0006 already exists, and it must rollback T-0005
    assert!(db.apply_engineering_lead_response(pid, &response).is_err());

    // reopen and verify T-0005 was not created
    let db2 = Database::open(&path).unwrap();
    assert!(db2.get_task("T-0005").unwrap().is_none());

    // pre-existing T-0006 should still be present
    assert!(db2.get_task("T-0006").unwrap().is_some());
}

#[test]
fn task_dependency_crud_and_validation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).expect("init");
    let pid = db.create_project("testproj").expect("project");

    let t1 = db
        .insert_task(pid, "T1", "First task", "dev", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(pid, "T2", "Second task", "dev", TaskPriority::Normal)
        .unwrap();
    let t3 = db
        .insert_task(pid, "T3", "Third task", "dev", TaskPriority::Normal)
        .unwrap();

    // self-dependency is rejected
    assert!(db.add_task_dependency(&t1, &t1).is_err());

    // missing task is rejected
    assert!(db.add_task_dependency(&t1, "T-9999").is_err());
    assert!(db.add_task_dependency("T-9999", &t1).is_err());

    // valid dependency
    db.add_task_dependency(&t2, &t1).expect("add t2 -> t1");
    assert_eq!(db.list_task_dependencies(&t2).unwrap(), vec![t1.clone()]);
    assert_eq!(db.list_task_dependents(&t1).unwrap(), vec![t2.clone()]);

    // duplicate dependency is rejected
    assert!(db.add_task_dependency(&t2, &t1).is_err());

    // cycle: t1 -> t2 when t2 -> t1
    assert!(db.add_task_dependency(&t1, &t2).is_err());

    // multi-step cycle: t3 -> t2 -> t1; trying to add t1 -> t3
    db.add_task_dependency(&t3, &t2).expect("add t3 -> t2");
    assert!(db.add_task_dependency(&t1, &t3).is_err());

    // remove dependency
    assert!(db.remove_task_dependency(&t2, &t1).unwrap());
    assert!(!db.remove_task_dependency(&t2, &t1).unwrap());
    assert!(db.list_task_dependencies(&t2).unwrap().is_empty());
}

#[test]
fn task_dependency_persistence_survives_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");

    let (t1, t2) = {
        let db = Database::init(&path).expect("init");
        let pid = db.create_project("testproj").expect("project");
        let t1 = db
            .insert_task(pid, "T1", "First", "dev", TaskPriority::Normal)
            .unwrap();
        let t2 = db
            .insert_task(pid, "T2", "Second", "dev", TaskPriority::Normal)
            .unwrap();
        db.add_task_dependency(&t2, &t1).unwrap();
        (t1, t2)
    };

    let db2 = Database::open(&path).expect("open");
    assert_eq!(db2.list_task_dependencies(&t2).unwrap(), vec![t1.clone()]);
    assert_eq!(db2.list_task_dependents(&t1).unwrap(), vec![t2]);
}

#[test]
fn exact_resolution_is_persisted_once_and_survives_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let run = {
        let db = Database::init(&path).unwrap();
        let project = db.create_project("economy").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let run = db
            .create_agent_run_with_execution(
                project,
                &task,
                "selected-agent",
                AUTOMATED,
                AgentRunExecution {
                    class: "coder",
                    model: Some("small-model"),
                    effort: Some(ReasoningEffort::Medium),
                    source: "operator_override",
                },
            )
            .unwrap();
        let record = ResolutionRecord {
            selected_agent: "selected-agent".into(),
            selected_model: Some("small-model".into()),
            effort: Some(ReasoningEffort::Medium),
            tier: EconomyTier::Default,
            source: "operator_override".into(),
            escalation_reason: None,
            input_lineage: r#"{"operator_model":"small-model"}"#.into(),
            escalation: None,
        };
        db.start_provider_invocation_with_resolution(run, "implementation", 1, &record)
            .unwrap();
        assert_eq!(db.resolution_records(run).unwrap(), vec![record]);
        run
    };

    let reopened = Database::open(&path).unwrap();
    let records = reopened.resolution_records(run).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, "operator_override");
    assert_eq!(records[0].tier, EconomyTier::Default);
    assert_eq!(records[0].selected_model.as_deref(), Some("small-model"));
}

#[test]
fn controller_origin_plan_is_proposed_without_legacy_lineage_or_tasks() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let db = Database::init(&path).unwrap();
    let project = db.create_project("controller plan").unwrap();
    let pending_decision = db
        .record_lead_decision(
            project,
            &LeadDecisionKind::PlanRequired,
            &serde_json::json!({"plan": "still pending"}),
            LeadDecisionMetadata {
                snapshot: "snapshot",
                run_id: None,
                source_request: "request",
                summary: "summary",
            },
        )
        .unwrap();
    let response = plan_response();

    let plan_id = db.store_controller_plan(project, &response).unwrap();
    let plan = db.get_plan(plan_id).unwrap().unwrap();
    assert_eq!(
        plan.provenance,
        orc::storage::db::PlanProvenance::controller()
    );
    assert_eq!(plan.status, orc::storage::db::PlanStatus::Proposed);
    assert_eq!(plan.version, 1);
    assert_eq!(plan.parent_plan_id, None);
    assert!(db.is_current_valid_plan(project, &plan).unwrap());
    assert_eq!(
        db.pending_lead_decision(project).unwrap().unwrap().id,
        pending_decision
    );
    assert!(db.list_tasks().unwrap().is_empty());
    assert_eq!(
        db.list_plan_history(project).unwrap()[0].provenance,
        orc::storage::db::PlanProvenance::controller()
    );

    let connection = Connection::open(&path).unwrap();
    let row = connection
        .query_row(
            "SELECT origin, source_lead_decision_id, source_planner_run_id FROM plans WHERE id = ?1",
            [plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row, ("controller".into(), None, None));
}

#[test]
fn legacy_plan_schema_migrates_and_reopens_with_truthful_lineage() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let (project, decision, run, plan_id) = {
        let db = Database::init(&path).unwrap();
        let project = db.create_project("legacy migration").unwrap();
        let task = db
            .insert_task(
                project,
                "existing",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let decision = db
            .record_lead_decision(
                project,
                &LeadDecisionKind::PlanRequired,
                &serde_json::json!({"plan": "needed"}),
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
        let plan_id = db
            .store_plan(project, decision, run, &plan_response())
            .unwrap();
        (project, decision, run, plan_id)
    };

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE plans_legacy (
                 id INTEGER PRIMARY KEY,
                 project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 version INTEGER NOT NULL,
                 parent_plan_id INTEGER REFERENCES plans(id),
                 source_lead_decision_id INTEGER NOT NULL REFERENCES lead_decisions(id),
                 source_planner_run_id INTEGER NOT NULL REFERENCES agent_runs(id),
                 status TEXT NOT NULL DEFAULT 'proposed',
                 response TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                 superseded_by_plan_id INTEGER REFERENCES plans(id)
             );
             INSERT INTO plans_legacy (id, project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response, created_at, superseded_by_plan_id)
                 SELECT id, project_id, version, parent_plan_id, source_lead_decision_id, source_planner_run_id, status, response, created_at, superseded_by_plan_id FROM plans;
             UPDATE lead_decisions SET status='consumed' WHERE id=(SELECT source_lead_decision_id FROM plans_legacy LIMIT 1);
             UPDATE agent_runs SET status='completed' WHERE id=(SELECT source_planner_run_id FROM plans_legacy LIMIT 1);
             DROP TABLE plans;
             ALTER TABLE plans_legacy RENAME TO plans;
             CREATE UNIQUE INDEX plans_project_version ON plans(project_id, version);
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();
    drop(connection);

    let reopened = Database::open(&path).unwrap();
    let plan = reopened.get_plan(plan_id).unwrap().unwrap();
    assert_eq!(
        plan.provenance,
        orc::storage::db::PlanProvenance::legacy(decision, run)
    );
    assert!(reopened.is_current_valid_plan(project, &plan).unwrap());
    assert_eq!(reopened.list_plan_dependencies(plan_id).unwrap().len(), 1);
    let history = reopened.list_plan_history(project).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].provenance, plan.provenance);
    let second_reopen = Database::open(&path).unwrap();
    assert_eq!(
        second_reopen.get_plan(plan_id).unwrap().unwrap().provenance,
        orc::storage::db::PlanProvenance::legacy(decision, run)
    );
    let connection = Connection::open(&path).unwrap();
    let origin: String = connection
        .query_row("SELECT origin FROM plans WHERE id = ?1", [plan_id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(origin, "legacy_planner");
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(0))
        .optional()
        .unwrap()
        .unwrap_or(0);
    assert_eq!(foreign_keys, 0);
}

#[test]
fn legacy_plan_review_schema_migrates_without_losing_lead_lineage() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("orc.db");
    let (project, plan_id, run_id, decision_id, second_review_id) = {
        let db = Database::init(&path).unwrap();
        let project = db.create_project("legacy review migration").unwrap();
        let task = db
            .insert_task(
                project,
                "existing",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        let decision = db
            .record_lead_decision(
                project,
                &LeadDecisionKind::PlanRequired,
                &serde_json::json!({"plan":"needed"}),
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
        let plan_id = db
            .store_plan(project, decision, run, &plan_response())
            .unwrap();
        let first_review_id = db
            .record_plan_review(
                plan_id,
                run,
                decision,
                &LeadDecisionKind::Approve,
                "legacy review",
            )
            .unwrap();
        let second_decision = db
            .record_lead_decision(
                project,
                &LeadDecisionKind::Approve,
                &serde_json::json!({"plan":"review again"}),
                LeadDecisionMetadata {
                    snapshot: "snapshot",
                    run_id: Some(run),
                    source_request: "request",
                    summary: "summary",
                },
            )
            .unwrap();
        let second_review_id = db
            .record_plan_review(
                plan_id,
                run,
                second_decision,
                &LeadDecisionKind::Approve,
                "second legacy review",
            )
            .unwrap();
        assert_ne!(first_review_id, second_review_id);
        (project, plan_id, run, decision, second_review_id)
    };

    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE plan_reviews_legacy (
                 id INTEGER PRIMARY KEY,
                 plan_id INTEGER NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
                 lead_run_id INTEGER NOT NULL REFERENCES agent_runs(id),
                 lead_decision_id INTEGER NOT NULL REFERENCES lead_decisions(id),
                 decision TEXT NOT NULL,
                 details TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (CURRENT_TIMESTAMP),
                 superseded_by_review_id INTEGER REFERENCES plan_reviews(id)
             );
             INSERT INTO plan_reviews_legacy (id, plan_id, lead_run_id, lead_decision_id, decision, details, created_at, superseded_by_review_id)
                 SELECT id, plan_id, lead_run_id, lead_decision_id, decision, details, created_at, superseded_by_review_id FROM plan_reviews;
             DROP TABLE plan_reviews;
             ALTER TABLE plan_reviews_legacy RENAME TO plan_reviews;
             PRAGMA foreign_keys=ON;",
        )
        .unwrap();
    drop(connection);

    let reopened = Database::open(&path).unwrap();
    let reviews = reopened.list_plan_reviews(project).unwrap();
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0].plan_id, plan_id);
    assert_eq!(reviews[0].lead_run_id, Some(run_id));
    assert_eq!(reviews[0].lead_decision_id, Some(decision_id));
    assert_eq!(reviews[0].superseded_by_review_id, Some(second_review_id));
    assert_eq!(
        reviews[0].origin,
        orc::storage::db::PlanReviewOrigin::LegacyLead
    );
    assert_eq!(
        reviews[0].decision,
        orc::storage::db::PlanReviewDecision::Approve
    );
    assert_eq!(reviews[1].id, second_review_id);
    assert!(reviews[1].superseded_by_review_id.is_none());
    let connection = Connection::open(&path).unwrap();
    let foreign_keys: i64 = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(0))
        .optional()
        .unwrap()
        .unwrap_or(0);
    assert_eq!(foreign_keys, 0);
}
