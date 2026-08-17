use orc::agent;
use orc::storage::Database;
use orc::task::TaskStatus;
use orc::worker::test_helpers::{FailingSpawnWorker, FakeWorker};
use tempfile::tempdir;

#[test]
fn active_task_cannot_be_dispatched_again() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Set task to active
    db.update_task_status(&tid, TaskStatus::Active)
        .expect("set active");

    // Try to dispatch active task
    let worker = FakeWorker::new_success(None);
    let result = agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already active"));
}

#[test]
fn done_task_cannot_be_dispatched() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Set task to done
    db.update_task_status(&tid, TaskStatus::Done)
        .expect("set done");

    // Try to dispatch done task
    let worker = FakeWorker::new_success(None);
    let result = agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already done"));
}

#[test]
fn successful_worker_transitions_active_to_review() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Initial status should be backlog
    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);

    // Dispatch with successful worker
    let worker = FakeWorker::new_success(None);
    let result = agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap());
    assert!(
        result.is_ok(),
        "dispatch should succeed: {:#?}",
        result.err()
    );

    // Verify task transitioned to review
    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Review);

    // Verify agent run was created and marked completed
    let runs = db.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "completed");
}

#[test]
fn failed_worker_transitions_active_to_blocked() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Dispatch with failing worker
    let worker = FakeWorker::new_failure("something went wrong".to_string());
    let result = agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap());
    assert!(result.is_err());

    // Verify task transitioned to blocked
    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);

    // Verify agent run was created and marked failed
    let runs = db.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "failed");
    assert!(runs[0].output.is_some());
}

#[test]
fn failed_spawn_does_not_leave_task_active() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Dispatch with worker that fails at spawn
    let worker = FailingSpawnWorker;
    let result = agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap());
    assert!(result.is_err());

    // Verify task is NOT active (should be blocked)
    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);

    // Verify agent run was created and marked failed
    let runs = db.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "failed");
}

#[test]
fn agent_run_status_output_timestamps_persist() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Dispatch with successful worker
    let output_text = "Deployment successful".to_string();
    let worker = FakeWorker::new_success(Some(output_text.clone()));
    agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap())
        .expect("dispatch should succeed");

    // Verify agent run has all expected fields
    let runs = db.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];

    assert_eq!(run.status, "completed");
    assert_eq!(run.output, Some(output_text));
    assert_eq!(run.agent, "copilot");
    assert_eq!(run.task_id, Some(tid));
    assert!(!run.started_at.is_empty());
    assert!(run.finished_at.is_some());
}

#[test]
fn reopening_db_preserves_run_history() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let tid = {
        let db = Database::init(&db_path).expect("init");
        let pid = db.create_project("test").expect("create project");
        let tid = db
            .insert_task(
                pid,
                "Test Task",
                "Do something",
                "dev",
                orc::task::TaskPriority::Normal,
            )
            .expect("insert task");

        // Dispatch with successful worker
        let worker = FakeWorker::new_success(Some("output".to_string()));
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap())
            .expect("dispatch should succeed");

        tid
    };

    // Reopen DB and verify run history is preserved
    let db2 = Database::open(&db_path).expect("reopen");
    let runs = db2.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "completed");
    assert_eq!(runs[0].output, Some("output".to_string()));
}

#[test]
fn task_transitions_through_lifecycle() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // Initial: backlog
    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);

    // After successful dispatch: review
    let worker = FakeWorker::new_success(None);
    agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap())
        .expect("dispatch should succeed");

    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[test]
fn multiple_runs_per_task_are_tracked() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("orc.db");

    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = db
        .insert_task(
            pid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

    // First successful run
    let worker = FakeWorker::new_success(None);
    agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap())
        .expect("first dispatch");

    // Reset task to backlog for another run
    db.update_task_status(&tid, TaskStatus::Backlog)
        .expect("reset to backlog");

    // Second run that fails
    let worker = FakeWorker::new_failure("failed".to_string());
    let _ = agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap());

    // Verify both runs are tracked
    let runs = db.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 2);
    // Most recent first (DESC ordering by started_at)
    assert_eq!(runs[0].status, "failed");
    assert_eq!(runs[1].status, "completed");
}
