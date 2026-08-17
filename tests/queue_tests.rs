use orc::queue::{BlockingReason, QueueCategory, compute_queue};
use orc::registry::{self, AgentDefinition};
use orc::storage::{Database, DbError};
use orc::task::{TaskPriority, TaskStatus};
use tempfile::tempdir;

fn create_test_db() -> (tempfile::TempDir, Database, i64) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");
    let db = Database::init(&db_path).unwrap();
    let pid = db.create_project("testproj").unwrap();
    (dir, db, pid)
}

fn add_agent(
    db: &Database,
    id: &str,
    backend: &str,
    mode: &str,
    priority: i64,
    capabilities: Vec<&str>,
) {
    let agent = AgentDefinition {
        id: id.to_string(),
        backend: backend.to_string(),
        display_name: id.to_string(),
        enabled: true,
        priority,
        capabilities: capabilities.into_iter().map(String::from).collect(),
        status: registry::AVAILABLE.to_string(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        execution_mode: mode.to_string(),
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
    };
    db.insert_agent(&agent).unwrap();
}

#[test]
fn task_with_no_dependencies_can_become_ready() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );

    let t1 = db
        .insert_task(
            pid,
            "Implement feature",
            "Do feature",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();

    let report = compute_queue(&db).unwrap();
    assert_eq!(report.ready.len(), 1);
    assert_eq!(report.ready[0].task.id, t1);
    assert_eq!(
        report.ready[0].recommended_agent.as_deref(),
        Some("codex-main")
    );
    assert_eq!(report.ready[0].category, QueueCategory::Ready);
    assert!(report.blocked.is_empty());
}

#[test]
fn dependency_on_unfinished_task_blocks_it() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );

    let t1 = db
        .insert_task(pid, "Base", "Base task", "developer", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(
            pid,
            "Followup",
            "Followup task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();

    db.add_task_dependency(&t2, &t1).unwrap();

    let report = compute_queue(&db).unwrap();
    assert_eq!(report.ready.len(), 1);
    assert_eq!(report.ready[0].task.id, t1);

    assert_eq!(report.blocked.len(), 1);
    assert_eq!(report.blocked[0].task.id, t2);
    assert_eq!(report.blocked[0].waiting_on, vec![t1.clone()]);
    assert_eq!(report.blocked[0].category, QueueCategory::Blocked);
    assert_eq!(
        report.blocked[0].blocking_reasons,
        vec![BlockingReason::DependencyBlocked {
            incomplete_dependencies: vec![orc::DependencyInfo {
                task_id: t1.clone(),
                status: Some(TaskStatus::Backlog),
                is_done: false,
            }],
        }]
    );
}

#[test]
fn dependency_on_done_task_allows_it() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );

    let t1 = db
        .insert_task(pid, "Base", "Base task", "developer", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(
            pid,
            "Followup",
            "Followup task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();

    db.add_task_dependency(&t2, &t1).unwrap();
    db.update_task_status(&t1, TaskStatus::Done).unwrap();

    let report = compute_queue(&db).unwrap();
    assert_eq!(report.done.len(), 1);
    assert_eq!(report.done[0].task.id, t1);

    assert_eq!(report.ready.len(), 1);
    assert_eq!(report.ready[0].task.id, t2);
    assert_eq!(
        report.ready[0].recommended_agent.as_deref(),
        Some("codex-main")
    );
    assert!(report.blocked.is_empty());
}

#[test]
fn multiple_dependencies_require_all_done() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );

    let t1 = db
        .insert_task(pid, "Dep1", "First dep", "developer", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(pid, "Dep2", "Second dep", "developer", TaskPriority::Normal)
        .unwrap();
    let t3 = db
        .insert_task(pid, "Main", "Main task", "developer", TaskPriority::Normal)
        .unwrap();

    db.add_task_dependency(&t3, &t1).unwrap();
    db.add_task_dependency(&t3, &t2).unwrap();

    // t1 done, t2 still backlog -> t3 blocked
    db.update_task_status(&t1, TaskStatus::Done).unwrap();
    let report1 = compute_queue(&db).unwrap();
    let t3_item = report1.blocked.iter().find(|i| i.task.id == t3).unwrap();
    assert_eq!(t3_item.waiting_on, vec![t2.clone()]);

    // t2 also done -> t3 becomes ready
    db.update_task_status(&t2, TaskStatus::Done).unwrap();
    let report2 = compute_queue(&db).unwrap();
    assert!(report2.blocked.is_empty());
    assert_eq!(report2.ready.len(), 1);
    assert_eq!(report2.ready[0].task.id, t3);
}

#[test]
fn self_dependency_rejected() {
    let (_dir, db, pid) = create_test_db();
    let t1 = db
        .insert_task(pid, "T1", "Task 1", "developer", TaskPriority::Normal)
        .unwrap();

    let err = db.add_task_dependency(&t1, &t1).unwrap_err();
    match err {
        DbError::SelfDependency(id) => assert_eq!(id, t1),
        other => panic!("expected SelfDependency, got {other:?}"),
    }
}

#[test]
fn duplicate_dependency_rejected() {
    let (_dir, db, pid) = create_test_db();
    let t1 = db
        .insert_task(pid, "T1", "Task 1", "developer", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(pid, "T2", "Task 2", "developer", TaskPriority::Normal)
        .unwrap();

    db.add_task_dependency(&t2, &t1).unwrap();
    let err = db.add_task_dependency(&t2, &t1).unwrap_err();
    match err {
        DbError::DuplicateDependency(a, b) => {
            assert_eq!(a, t2);
            assert_eq!(b, t1);
        }
        other => panic!("expected DuplicateDependency, got {other:?}"),
    }
}

#[test]
fn missing_dependency_rejected() {
    let (_dir, db, pid) = create_test_db();
    let t1 = db
        .insert_task(pid, "T1", "Task 1", "developer", TaskPriority::Normal)
        .unwrap();

    let err1 = db.add_task_dependency(&t1, "T-9999").unwrap_err();
    match err1 {
        DbError::TaskNotFound(id) => assert_eq!(id, "T-9999"),
        other => panic!("expected TaskNotFound, got {other:?}"),
    }

    let err2 = db.add_task_dependency("T-9999", &t1).unwrap_err();
    match err2 {
        DbError::TaskNotFound(id) => assert_eq!(id, "T-9999"),
        other => panic!("expected TaskNotFound, got {other:?}"),
    }
}

#[test]
fn simple_cycle_rejected() {
    let (_dir, db, pid) = create_test_db();
    let t1 = db
        .insert_task(pid, "T1", "Task 1", "developer", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(pid, "T2", "Task 2", "developer", TaskPriority::Normal)
        .unwrap();
    let t3 = db
        .insert_task(pid, "T3", "Task 3", "developer", TaskPriority::Normal)
        .unwrap();

    // 2-node cycle: T2 -> T1; T1 -> T2
    db.add_task_dependency(&t2, &t1).unwrap();
    let err = db.add_task_dependency(&t1, &t2).unwrap_err();
    match err {
        DbError::DependencyCycle(a, b) => {
            assert_eq!(a, t1);
            assert_eq!(b, t2);
        }
        other => panic!("expected DependencyCycle, got {other:?}"),
    }

    // 3-node cycle: T3 -> T2 -> T1; T1 -> T3
    db.add_task_dependency(&t3, &t2).unwrap();
    let err3 = db.add_task_dependency(&t1, &t3).unwrap_err();
    match err3 {
        DbError::DependencyCycle(a, b) => {
            assert_eq!(a, t1);
            assert_eq!(b, t3);
        }
        other => panic!("expected DependencyCycle, got {other:?}"),
    }
}

#[test]
fn active_review_done_tasks_are_not_ready() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );

    let t_done = db
        .insert_task(pid, "Done", "Done task", "developer", TaskPriority::Normal)
        .unwrap();
    db.update_task_status(&t_done, TaskStatus::Done).unwrap();

    let t_review = db
        .insert_task(
            pid,
            "Review",
            "Review task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&t_review, TaskStatus::Review)
        .unwrap();

    let t_active = db
        .insert_task(
            pid,
            "Active",
            "Active task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&t_active, TaskStatus::Active)
        .unwrap();

    let t_blocked = db
        .insert_task(
            pid,
            "Blocked",
            "Blocked task",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&t_blocked, TaskStatus::Blocked)
        .unwrap();

    let report = compute_queue(&db).unwrap();
    assert!(report.ready.is_empty());
    assert_eq!(report.done.len(), 1);
    assert_eq!(report.done[0].task.id, t_done);
    assert_eq!(report.review.len(), 1);
    assert_eq!(report.review[0].task.id, t_review);
    assert_eq!(report.active.len(), 1);
    assert_eq!(report.active[0].task.id, t_active);
    assert_eq!(report.blocked.len(), 1);
    assert_eq!(report.blocked[0].task.id, t_blocked);
    assert_eq!(
        report.blocked[0].blocking_reasons,
        vec![BlockingReason::PersistedLifecycleBlocked]
    );
}

#[test]
fn no_eligible_agent_makes_task_unrunnable_with_explanation() {
    let (_dir, db, pid) = create_test_db();
    // Agent only has "architecture" capability
    add_agent(
        &db,
        "claude-arch",
        "claude",
        "manual",
        100,
        vec!["architecture"],
    );

    // Developer task requires "code" and "terminal"
    let t1 = db
        .insert_task(
            pid,
            "Implement engine",
            "Coding engine",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();

    let report = compute_queue(&db).unwrap();
    assert!(report.ready.is_empty());
    assert_eq!(report.backlog.len(), 1);
    assert_eq!(report.backlog[0].task.id, t1);
    assert_eq!(report.backlog[0].category, QueueCategory::Backlog);
    match &report.backlog[0].blocking_reasons[0] {
        BlockingReason::NoEligibleAgent {
            explanation,
            rejections,
        } => {
            assert!(explanation.contains("No eligible agent satisfies requirements"));
            assert_eq!(rejections.len(), 1);
        }
        other => panic!("expected NoEligibleAgent, got {other:?}"),
    }
    assert!(report.backlog[0].schedule_decision.is_some());
}

#[test]
fn manual_agent_can_make_reasoning_task_ready() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "claude-manual",
        "claude",
        "manual",
        100,
        vec!["architecture"],
    );

    let t1 = db
        .insert_task(
            pid,
            "Design architecture",
            "Design system",
            "architect",
            TaskPriority::Normal,
        )
        .unwrap();

    let report = compute_queue(&db).unwrap();
    assert_eq!(report.ready.len(), 1);
    assert_eq!(report.ready[0].task.id, t1);
    assert_eq!(
        report.ready[0].recommended_agent.as_deref(),
        Some("claude-manual")
    );
    assert!(report.backlog.is_empty());
}

#[test]
fn queue_classification_is_deterministic() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    add_agent(
        &db,
        "claude-manual",
        "claude",
        "manual",
        50,
        vec!["architecture"],
    );

    let t1 = db
        .insert_task(pid, "T1", "Task 1", "developer", TaskPriority::Normal)
        .unwrap();
    let t2 = db
        .insert_task(pid, "T2", "Task 2", "developer", TaskPriority::Normal)
        .unwrap();
    let _t3 = db
        .insert_task(pid, "T3", "Task 3", "architect", TaskPriority::Normal)
        .unwrap();
    db.add_task_dependency(&t2, &t1).unwrap();

    let report1 = compute_queue(&db).unwrap();
    let report2 = compute_queue(&db).unwrap();

    assert_eq!(report1, report2);
    assert_eq!(report1.format_concise(), report2.format_concise());
    assert_eq!(report1.format_explain(), report2.format_explain());
}

#[test]
fn dependency_persistence_survives_db_reopen() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let (t1, t2) = {
        let db = Database::init(&db_path).unwrap();
        let pid = db.create_project("testproj").unwrap();
        add_agent(
            &db,
            "codex-main",
            "codex",
            "automated",
            100,
            vec!["code", "terminal"],
        );
        let t1 = db
            .insert_task(pid, "T1", "Task 1", "developer", TaskPriority::Normal)
            .unwrap();
        let t2 = db
            .insert_task(pid, "T2", "Task 2", "developer", TaskPriority::Normal)
            .unwrap();
        db.add_task_dependency(&t2, &t1).unwrap();
        (t1, t2)
    };

    let db_reopened = Database::open(&db_path).unwrap();
    let report = compute_queue(&db_reopened).unwrap();
    assert_eq!(report.ready.len(), 1);
    assert_eq!(report.ready[0].task.id, t1);
    assert_eq!(report.blocked.len(), 1);
    assert_eq!(report.blocked[0].task.id, t2);
}

#[test]
fn queue_does_not_invoke_real_providers() {
    let (_dir, db, pid) = create_test_db();
    // Configure an agent pointing to a non-existent / unreachable backend or invalid profile
    add_agent(
        &db,
        "antigravity-1",
        "antigravity",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    let t1 = db
        .insert_task(pid, "Task", "Task obj", "developer", TaskPriority::Normal)
        .unwrap();

    // compute_queue must succeed completely offline using persisted state
    let report = compute_queue(&db).unwrap();
    assert_eq!(report.ready.len(), 1);
    assert_eq!(report.ready[0].task.id, t1);
}

#[test]
fn queue_concise_and_explain_formatting() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );

    let t1 = db
        .insert_task(
            pid,
            "Implement worker selection",
            "obj",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let t2 = db
        .insert_task(
            pid,
            "Add scheduler",
            "obj",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.add_task_dependency(&t2, &t1).unwrap();

    let report = compute_queue(&db).unwrap();
    let concise = report.format_concise();
    assert!(concise.contains("READY\n"));
    assert!(concise.contains(&format!("{t1}  Implement worker selection")));
    assert!(concise.contains("BLOCKED\n"));
    assert!(concise.contains(&format!("{t2}  Add scheduler")));
    assert!(concise.contains(&format!("waiting on: {t1}")));

    let explain = report.format_explain();
    assert!(explain.contains("=== READY ==="));
    assert!(explain.contains(&t1));
    assert!(explain.contains("Recommended agent:    codex-main"));
    assert!(explain.contains("=== BLOCKED ==="));
    assert!(explain.contains(&t2));
    assert!(explain.contains(&format!("incomplete dependencies: {t1} [backlog]")));
}
