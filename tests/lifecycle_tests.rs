use orc::agent;
use orc::storage::Database;
use orc::task::TaskStatus;
use orc::worker::test_helpers::{FailingSpawnWorker, FakeWorker};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn get_unique_task_id() -> String {
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("T-{:04}", id)
}

fn init_temp_git_repo(dir: &std::path::Path) {
    // Create .orc directory with engineering.md
    let orc_dir = dir.join(".orc");
    std::fs::create_dir_all(&orc_dir).expect("create .orc dir");
    std::fs::write(
        orc_dir.join("engineering.md"),
        "# Test Engineering Contract\n",
    )
    .expect("write engineering.md");
    std::fs::write(orc_dir.join("validation.toml"), "commands = []\n")
        .expect("write validation config");

    // Initialize a git repo in the temporary directory
    Command::new("git")
        .current_dir(dir)
        .arg("init")
        .arg(".")
        .output()
        .expect("init repo");

    // Configure git user for commit operations
    Command::new("git")
        .current_dir(dir)
        .arg("config")
        .arg("user.email")
        .arg("test@example.com")
        .output()
        .expect("config email");

    Command::new("git")
        .current_dir(dir)
        .arg("config")
        .arg("user.name")
        .arg("Test User")
        .output()
        .expect("config name");

    // Create initial commit
    let file_path = dir.join("README.md");
    std::fs::write(&file_path, "test").expect("write file");
    Command::new("git")
        .current_dir(dir)
        .arg("add")
        .arg(".")
        .output()
        .expect("git add");
    Command::new("git")
        .current_dir(dir)
        .arg("commit")
        .arg("-m")
        .arg("initial")
        .output()
        .expect("git commit");
}

#[test]
fn active_task_cannot_be_dispatched_again() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
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
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already active"));
}

#[test]
fn done_task_cannot_be_dispatched() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
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
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already done"));
}

#[test]
fn successful_worker_transitions_active_to_review() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    // Initial status should be backlog
    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Backlog);

    // Dispatch with worker in repo context
    let worker = FakeWorker::new_success(None);
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

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
    assert!(
        db.list_approval_requests(pid)
            .expect("list approvals")
            .is_empty()
    );
}

#[test]
fn architecture_decision_output_creates_approval_request() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    let worker = FakeWorker::new_success(Some(
        "Implemented the change.\nORC-ARCHITECTURE-DECISION: use the existing worker abstraction\nORC-ARCHITECTURE-DECISION: add a storage migration\nORC-ARCHITECTURE-DECISION: use the existing worker abstraction\n".into(),
    ));
    agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir)
        .expect("dispatch");

    let requests = db.list_approval_requests(pid).expect("list approvals");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].reason, "use the existing worker abstraction");
    assert_eq!(requests[1].reason, "add a storage migration");
    assert_eq!(
        db.get_task(&tid).expect("get task").unwrap().status,
        TaskStatus::Review
    );
}

#[test]
fn empty_and_inline_architecture_decisions_create_no_approval_requests() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    let worker = FakeWorker::new_success(Some(
        "ORC-ARCHITECTURE-DECISION:\ntext ORC-ARCHITECTURE-DECISION: ignored\nORC-ARCHITECTURE-DECISION:   \n".into(),
    ));
    agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir)
        .expect("dispatch");

    assert!(
        db.list_approval_requests(pid)
            .expect("list approvals")
            .is_empty()
    );
}

#[test]
fn failed_worker_transitions_active_to_blocked() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    // Dispatch with failing worker
    let worker = FakeWorker::new_failure("something went wrong".to_string());
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

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
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    // Dispatch with worker that fails at spawn
    let worker = FailingSpawnWorker;
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

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
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    // Dispatch with output text
    let output_text = "Deployment successful".to_string();
    let worker = FakeWorker::new_success(Some(output_text.clone()));
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

    assert!(result.is_ok(), "dispatch should succeed");

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
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");

    let tid = {
        let db = Database::init(&db_path).expect("init");
        let pid = db.create_project("test").expect("create project");
        let tid = get_unique_task_id();
        db.insert_task_with_id(
            pid,
            &tid,
            "Test Task",
            "Do something",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .expect("insert task");

        let worker = FakeWorker::new_success(Some("output".to_string()));
        let result =
            agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

        assert!(result.is_ok(), "dispatch should succeed");
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
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
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
    let result =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);

    assert!(result.is_ok(), "dispatch should succeed");

    let task = db.get_task(&tid).expect("get task").unwrap();
    assert_eq!(task.status, TaskStatus::Review);
}

#[test]
fn multiple_runs_per_task_are_tracked() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).expect("init");
    let pid = db.create_project("test").expect("create project");
    let tid = get_unique_task_id();
    db.insert_task_with_id(
        pid,
        &tid,
        "Test Task",
        "Do something",
        "dev",
        orc::task::TaskPriority::Normal,
    )
    .expect("insert task");

    // First successful run
    let worker = FakeWorker::new_success(None);
    let result1 =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);
    assert!(result1.is_ok(), "first dispatch should succeed");

    // Reset task to backlog for another run
    db.update_task_status(&tid, TaskStatus::Backlog)
        .expect("reset to backlog");

    // Second run that fails
    let worker = FakeWorker::new_failure("failed".to_string());
    let result2 =
        agent::dispatch_with_worker_and_db(&tid, &worker, db_path.to_str().unwrap(), &repo_dir);
    assert!(result2.is_err(), "second dispatch should fail");

    // Verify both runs are tracked
    let runs = db.list_agent_runs_for_task(&tid).expect("list runs");
    assert_eq!(runs.len(), 2);
    // Most recent first (DESC ordering by started_at)
    assert_eq!(runs[0].status, "failed");
    assert_eq!(runs[1].status, "completed");
}

#[test]
fn scoped_lifecycle_limits_are_applied_after_scoping() {
    let directory = TempDir::new().unwrap();
    let db = Database::init(directory.path().join("state.sqlite")).unwrap();
    let project = db.create_project("test").unwrap();
    let target = db
        .insert_task(
            project,
            "target",
            "objective",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let other = db
        .insert_task(
            project,
            "other",
            "objective",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let target_run = db.create_agent_run(project, &target, "agent").unwrap();
    let other_run = db.create_agent_run(project, &other, "agent").unwrap();
    db.record_lifecycle_event("other", Some(&other), Some(other_run), None, None)
        .unwrap();
    db.record_lifecycle_event("target_old", Some(&target), Some(target_run), None, None)
        .unwrap();
    db.record_lifecycle_event("target_new", Some(&target), Some(target_run), None, None)
        .unwrap();
    db.record_lifecycle_event("run_new", Some(&other), Some(target_run), None, None)
        .unwrap();

    let task_events = db.list_lifecycle_events_for_task(&target, 2).unwrap();
    assert_eq!(
        task_events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["target_new", "target_old"]
    );
    let run_events = db.list_lifecycle_events_for_run(target_run, 2).unwrap();
    assert_eq!(
        run_events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["run_new", "target_new"]
    );
}

#[test]
fn worker_output_is_activity_without_changing_semantic_phase() {
    let directory = TempDir::new().unwrap();
    let db = Database::init(directory.path().join("state.sqlite")).unwrap();
    let project = db.create_project("test").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "dev",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let run = db.create_agent_run(project, &task, "agent").unwrap();
    db.update_agent_run_phase(run, "executing").unwrap();
    db.record_worker_output(run, "line").unwrap();

    let events = db.list_lifecycle_events_for_run(run, 10).unwrap();
    assert_eq!(events[0].kind, "worker_output");
    assert_eq!(
        db.get_agent_run(run).unwrap().unwrap().phase.as_deref(),
        Some("executing")
    );
}
