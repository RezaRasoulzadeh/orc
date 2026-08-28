use orc::agent::plan_dispatch_assignments;
use orc::queue::{BlockingReason, QueueCategory, QueueEntry, compute_queue};
use orc::registry::{self, AgentDefinition};
use orc::scheduler::{CandidateStatus, RejectionReason, schedule_with_busy};
use orc::storage::{Database, DbError};
use orc::task::{TaskPriority, TaskStatus};
use std::collections::HashSet;
use std::sync::Mutex;
use tempfile::tempdir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn dispatch_entry(id: &str) -> QueueEntry {
    QueueEntry {
        task: orc::task::Task {
            id: id.to_string(),
            title: id.to_string(),
            objective: "test".to_string(),
            role: "developer".to_string(),
            priority: TaskPriority::Normal,
            status: TaskStatus::Ready,
            cancellation_reason: None,
            required_capabilities: vec!["code".into(), "terminal".into()],
            scope_mode: None,
            context_files: Vec::new(),
            expected_changes: Vec::new(),
            reasoning_effort: None,
            effort_reason: None,
            risk_factors: Vec::new(),
        },
        category: QueueCategory::Ready,
        dependencies: Vec::new(),
        waiting_on: Vec::new(),
        blocking_reasons: Vec::new(),
        active_agent: None,
        recommended_agent: None,
        schedule_decision: None,
        recommended_execution: None,
    }
}

fn dispatch_agent(id: &str) -> AgentDefinition {
    AgentDefinition {
        id: id.into(),
        backend: "codex".into(),
        display_name: id.into(),
        enabled: true,
        priority: 100,
        capabilities: ["code", "terminal"].into_iter().map(String::from).collect(),
        status: registry::AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        execution_mode: registry::AUTOMATED.into(),
        quota_remaining_percent: Some(100),
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![registry::AgentAction::Code],
    }
}

#[test]
fn dispatch_planning_respects_numeric_limit_and_deterministic_auto_capacity() {
    let ready = [
        dispatch_entry("T-0002"),
        dispatch_entry("T-0001"),
        dispatch_entry("T-0003"),
    ];
    let agents = [dispatch_agent("agent-a"), dispatch_agent("agent-b")];
    let busy = HashSet::new();
    assert_eq!(
        plan_dispatch_assignments(&ready, &agents, &busy, 10, Some(1))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        plan_dispatch_assignments(&ready, &agents, &busy, 10, None).unwrap(),
        vec![
            ("T-0002".into(), "agent-a".into()),
            ("T-0001".into(), "agent-b".into())
        ]
    );
}

#[test]
fn dispatch_planning_excludes_unusable_agents_and_reuses_none() {
    let ready = [dispatch_entry("T-0001"), dispatch_entry("T-0002")];
    let mut disabled = dispatch_agent("disabled");
    disabled.enabled = false;
    let mut unavailable = dispatch_agent("unavailable");
    unavailable.status = "offline".into();
    let mut exhausted = dispatch_agent("exhausted");
    exhausted.quota_remaining_percent = Some(0);
    let mut reserve = dispatch_agent("reserve");
    reserve.quota_remaining_percent = Some(5);
    let agents = [
        disabled,
        unavailable,
        exhausted,
        reserve,
        dispatch_agent("usable"),
        dispatch_agent("other"),
    ];
    let busy = HashSet::from(["other".to_string()]);
    assert_eq!(
        plan_dispatch_assignments(&ready, &agents, &busy, 10, None).unwrap(),
        vec![("T-0001".into(), "usable".into())]
    );
}

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
        actions: vec![registry::AgentAction::Code],
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
fn dispatch_eligibility_rejects_dependency_and_persisted_blocks() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    let dependency = db
        .insert_task(
            pid,
            "Prerequisite",
            "finish first",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let dependent = db
        .insert_task(
            pid,
            "Dependent",
            "wait for prerequisite",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    db.add_task_dependency(&dependent, &dependency).unwrap();
    assert!(
        compute_queue(&db)
            .unwrap()
            .blocked
            .iter()
            .any(|entry| entry.task.id == dependent)
    );
    assert!(orc::queue::ensure_dispatchable(&db, &dependent).is_err());

    db.update_task_status(&dependent, TaskStatus::Blocked)
        .unwrap();
    assert!(orc::queue::ensure_dispatchable(&db, &dependent).is_err());

    db.update_task_status(&dependent, TaskStatus::Backlog)
        .unwrap();
    assert!(orc::queue::ensure_dispatchable(&db, &dependent).is_err());
    db.update_task_status(&dependency, TaskStatus::Done)
        .unwrap();
    assert!(orc::queue::ensure_dispatchable(&db, &dependent).is_ok());
}

#[test]
fn cancelled_task_is_explicit_and_not_ready() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "codex-main",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    let task_id = db
        .insert_task(
            pid,
            "Abandoned",
            "No longer needed",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    assert!(db.cancel_task(&task_id, Some("superseded")).unwrap());

    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Cancelled);
    assert_eq!(task.cancellation_reason.as_deref(), Some("superseded"));
    let report = compute_queue(&db).unwrap();
    assert_eq!(report.cancelled[0].task.id, task_id);
    assert!(report.ready.is_empty());
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
fn schedule_with_busy_rejects_busy_agent() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "busy",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    add_agent(
        &db,
        "free",
        "codex",
        "automated",
        90,
        vec!["code", "terminal"],
    );
    let task = db
        .insert_task(pid, "Task", "obj", "developer", TaskPriority::Normal)
        .unwrap();
    let busy = HashSet::from(["busy".to_string()]);
    let decision = schedule_with_busy(
        &db.get_task(&task).unwrap().unwrap(),
        &db.list_agents().unwrap(),
        Some(registry::AUTOMATED),
        &busy,
    )
    .unwrap();
    assert_eq!(decision.selected_agent_id.as_deref(), Some("free"));
    let busy_candidate = decision
        .candidates
        .iter()
        .find(|candidate| candidate.agent_id == "busy")
        .unwrap();
    assert_eq!(
        busy_candidate.status,
        CandidateStatus::Rejected(RejectionReason::Busy)
    );
}

#[test]
fn dispatch_batch_reservation_is_deterministic_and_capacity_limited() {
    let (_dir, db, pid) = create_test_db();
    add_agent(
        &db,
        "agent-a",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    add_agent(
        &db,
        "agent-b",
        "codex",
        "automated",
        100,
        vec!["code", "terminal"],
    );
    add_agent(
        &db,
        "manual",
        "human",
        "manual",
        100,
        vec!["code", "terminal"],
    );
    let first = db
        .insert_task(pid, "First", "obj", "developer", TaskPriority::Normal)
        .unwrap();
    let second = db
        .insert_task(pid, "Second", "obj", "developer", TaskPriority::Normal)
        .unwrap();
    let agents = db.list_agents().unwrap();
    let mut reserved = HashSet::new();
    let mut assignments = Vec::new();
    for task_id in [first.clone(), second.clone()] {
        let decision = schedule_with_busy(
            &db.get_task(&task_id).unwrap().unwrap(),
            &agents,
            Some(registry::AUTOMATED),
            &reserved,
        )
        .unwrap();
        let agent = decision.selected_agent_id.unwrap();
        reserved.insert(agent.clone());
        assignments.push((task_id, agent));
    }
    assert_eq!(
        assignments,
        vec![(first, "agent-a".into()), (second, "agent-b".into())]
    );
    assert_eq!(reserved.len(), 2);
    assert!(!reserved.contains("manual"));
}

#[test]
fn queue_excludes_active_agent_and_reports_only_available_tasks() {
    let (_dir, db, pid) = create_test_db();
    add_agent(&db, "active", "codex", "automated", 100, vec!["code"]);
    let task = db
        .insert_task(pid, "Task", "obj", "developer", TaskPriority::Normal)
        .unwrap();
    db.create_agent_run(pid, &task, "active").unwrap();
    let report = compute_queue(&db).unwrap();
    assert!(report.ready.is_empty());
    assert!(report.backlog.iter().any(|item| {
        item.task.id == task
            && item
                .blocking_reasons
                .iter()
                .any(|reason| matches!(reason, BlockingReason::NoEligibleAgent { .. }))
    }));
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

#[test]
fn queue_display_sections_follow_requested_order_without_reordering_items() {
    let report = orc::queue::QueueReport {
        ready: vec![dispatch_entry("ready-1"), dispatch_entry("ready-2")],
        blocked: vec![dispatch_entry("blocked")],
        active: vec![dispatch_entry("active")],
        review: vec![dispatch_entry("review")],
        backlog: vec![dispatch_entry("backlog")],
        done: vec![dispatch_entry("done")],
        cancelled: vec![dispatch_entry("cancelled")],
    };

    assert_eq!(
        report
            .all_items()
            .into_iter()
            .map(|entry| entry.task.id.as_str())
            .collect::<Vec<_>>(),
        [
            "cancelled",
            "done",
            "blocked",
            "backlog",
            "ready-1",
            "ready-2",
            "review",
            "active"
        ]
    );

    let concise = report.format_concise();
    let concise_sections = [
        "CANCELLED",
        "DONE",
        "BLOCKED",
        "BACKLOG",
        "READY",
        "REVIEW",
        "ACTIVE",
    ]
    .map(|section| concise.find(section).unwrap());
    assert!(concise_sections.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(concise.find("ready-1").unwrap() < concise.find("ready-2").unwrap());

    let explain = report.format_explain();
    let explain_sections = [
        "CANCELLED",
        "DONE",
        "BLOCKED",
        "BACKLOG",
        "READY",
        "REVIEW",
        "ACTIVE",
    ]
    .map(|section| explain.find(&format!("=== {section} ===")).unwrap());
    assert!(explain_sections.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn queue_explain_shows_recommended_execution_template() {
    let _guard = ENV_LOCK.lock().unwrap();
    let old = std::env::var_os("ORC_CODER_MODEL");
    unsafe { std::env::remove_var("ORC_CODER_MODEL") };
    let (_dir, db, pid) = create_test_db();
    let agent = AgentDefinition {
        id: "recommended".into(),
        backend: "codex".into(),
        display_name: "recommended".into(),
        enabled: true,
        priority: 100,
        capabilities: ["code", "terminal"].into_iter().map(String::from).collect(),
        status: registry::AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: Some("agent-model".into()),
        reasoning_effort: Some(registry::ReasoningEffort::High),
        config_metadata: None,
        execution_mode: registry::AUTOMATED.into(),
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![registry::AgentAction::Code],
    };
    db.insert_agent(&agent).unwrap();
    db.set_execution_template(
        orc::execution::ExecutionClass::Coder,
        Some("persistent-model"),
        Some(registry::ReasoningEffort::Medium),
    )
    .unwrap();
    let task = db
        .insert_task(
            pid,
            "developer task",
            "objective",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let report = compute_queue(&db).unwrap();
    let explain = report.format_explain();
    assert_eq!(report.ready[0].task.id, task);
    assert!(explain.contains(
        "class=coder, model=persistent-model, effort=medium, source=persistent-template"
    ));
    match old {
        Some(value) => unsafe { std::env::set_var("ORC_CODER_MODEL", value) },
        None => unsafe { std::env::remove_var("ORC_CODER_MODEL") },
    }
}
