use orc::storage::{Database, WorkerResult};
use orc::task::{TaskPriority, TaskStatus};
use rusqlite::Connection;
use tempfile::tempdir;

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
        })
        .unwrap();
        assert!(
            db.insert_worker_result(&WorkerResult {
                run_id,
                outcome: "success".into(),
                failure_category: None,
                duration_ms: None,
                metadata: None,
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
        })
    );
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
