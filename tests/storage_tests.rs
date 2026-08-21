use orc::storage::Database;
use orc::task::{TaskPriority, TaskStatus};
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
