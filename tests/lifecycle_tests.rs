use orc::agent;
use orc::registry::{self, AgentAction, AgentDefinition};
use orc::storage::Database;
use orc::task::TaskStatus;
use orc::validation::test_helpers::FakeValidationRunner;
use orc::worker::test_helpers::{FailingSpawnWorker, FakeWorker};
use orc::worker::{Worker, WorkerOutcome};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct CountingWorker {
    calls: AtomicUsize,
}

impl CountingWorker {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }
}

impl Worker for CountingWorker {
    fn execute(&self, _: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(cwd.join("eligibility-change.txt"), "dispatched\n")
            .map_err(|error| error.to_string())?;
        Ok((WorkerOutcome::Success, Some("executed".into())))
    }
}

fn get_unique_task_id() -> String {
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("T-{:04}", id)
}

fn register_eligible_agent(db: &Database) {
    db.insert_agent(&AgentDefinition {
        id: "eligible-codex".into(),
        backend: "codex".into(),
        execution_mode: registry::AUTOMATED.into(),
        display_name: "Eligible Codex".into(),
        enabled: true,
        priority: 100,
        capabilities: vec!["code".into(), "terminal".into()],
        status: registry::AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        quota_remaining_percent: Some(100),
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![AgentAction::Code],
    })
    .expect("register eligible agent");
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
        register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    register_eligible_agent(&db);
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
    db.update_agent_run_status(target_run, "completed", None)
        .unwrap();
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
    register_eligible_agent(&db);
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

#[test]
fn worker_backed_dispatch_rejects_backlog_with_no_eligible_agent() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db = Database::init(repo.join("orc.db")).unwrap();
    let project = db.create_project("test").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let worker = CountingWorker::new();

    let result = agent::dispatch_with_worker_on_db(
        &task,
        &worker,
        &db,
        &repo,
        "injected",
        &FakeValidationRunner::success(),
    );

    assert!(result.is_err());
    assert_eq!(worker.calls.load(Ordering::SeqCst), 0);
    assert!(db.list_agent_runs_for_task(&task).unwrap().is_empty());
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Backlog
    );
    assert!(!repo.join(".orc/worktrees").join(&task).exists());
    assert!(
        db.list_lifecycle_events_for_task(&task, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn dependency_blocked_dispatch_rejected_with_explicit_agent() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db = Database::init(repo.join("orc.db")).unwrap();
    let project = db.create_project("test").unwrap();
    register_eligible_agent(&db);
    let dependency = db
        .insert_task(
            project,
            "dependency",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    db.add_task_dependency(&task, &dependency).unwrap();

    let result = agent::dispatch_selected_with_db_and_repo(
        &db,
        &repo,
        &task,
        Some("eligible-codex"),
        None,
        None,
    );

    assert!(result.is_err());
    assert!(db.list_agent_runs_for_task(&task).unwrap().is_empty());
    assert_ne!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Active
    );
    assert!(
        db.list_lifecycle_events_for_task(&task, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn persisted_blocked_dispatch_rejected_with_explicit_agent() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db = Database::init(repo.join("orc.db")).unwrap();
    let project = db.create_project("test").unwrap();
    register_eligible_agent(&db);
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&task, TaskStatus::Blocked).unwrap();

    let result = agent::dispatch_selected_with_db_and_repo(
        &db,
        &repo,
        &task,
        Some("eligible-codex"),
        None,
        None,
    );

    assert!(result.is_err());
    assert!(db.list_agent_runs_for_task(&task).unwrap().is_empty());
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Blocked
    );
}

#[test]
fn ready_task_dispatches_through_worker_backed_path() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db = Database::init(repo.join("orc.db")).unwrap();
    let project = db.create_project("test").unwrap();
    register_eligible_agent(&db);
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let worker = CountingWorker::new();

    agent::dispatch_with_worker_on_db(
        &task,
        &worker,
        &db,
        &repo,
        "eligible-codex",
        &FakeValidationRunner::success(),
    )
    .unwrap();

    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);
    assert_eq!(db.list_agent_runs_for_task(&task).unwrap().len(), 1);
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Review
    );
}

#[test]
fn queue_and_dispatch_are_consistent() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db = Database::init(repo.join("orc.db")).unwrap();
    let project = db.create_project("test").unwrap();
    register_eligible_agent(&db);
    let ready = db
        .insert_task(
            project,
            "ready",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let blocked = db
        .insert_task(
            project,
            "blocked",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    db.update_task_status(&blocked, TaskStatus::Blocked)
        .unwrap();
    let backlog = db
        .insert_task(
            project,
            "backlog",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    db.set_task_required_capabilities(&backlog, &["gpu".into()])
        .unwrap();
    let queue = orc::queue::compute_queue(&db).unwrap();
    assert!(queue.ready.iter().any(|entry| entry.task.id == ready));
    assert!(queue.blocked.iter().any(|entry| entry.task.id == blocked));
    assert!(queue.backlog.iter().any(|entry| entry.task.id == backlog));

    let worker = CountingWorker::new();
    agent::dispatch_with_worker_on_db(
        &ready,
        &worker,
        &db,
        &repo,
        "eligible-codex",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    assert!(
        agent::dispatch_with_worker_on_db(
            &blocked,
            &worker,
            &db,
            &repo,
            "eligible-codex",
            &FakeValidationRunner::success()
        )
        .is_err()
    );
    assert!(
        agent::dispatch_with_worker_on_db(
            &backlog,
            &worker,
            &db,
            &repo,
            "eligible-codex",
            &FakeValidationRunner::success()
        )
        .is_err()
    );
    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn injected_worker_never_changes_eligibility() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db = Database::init(repo.join("orc.db")).unwrap();
    let project = db.create_project("test").unwrap();
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let worker = CountingWorker::new();

    assert!(
        agent::dispatch_with_worker_on_db(
            &task,
            &worker,
            &db,
            &repo,
            "injected",
            &FakeValidationRunner::success()
        )
        .is_err()
    );
    assert!(
        agent::dispatch_selected_with_db_and_repo(&db, &repo, &task, None, None, None).is_err()
    );
    assert_eq!(worker.calls.load(Ordering::SeqCst), 0);
    assert!(db.list_agent_runs_for_task(&task).unwrap().is_empty());
}

#[test]
fn retryable_blocked_task_requires_requeue_before_dispatch() {
    let directory = TempDir::new().unwrap();
    let repo = directory.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_temp_git_repo(&repo);
    let db_path = repo.join("orc.db");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("test").unwrap();
    register_eligible_agent(&db);
    let task = db
        .insert_task(
            project,
            "task",
            "objective",
            "developer",
            orc::task::TaskPriority::Normal,
        )
        .unwrap();
    let failed_run = db
        .create_agent_run(project, &task, "eligible-codex")
        .unwrap();
    db.update_agent_run_status(failed_run, "failed", Some("worker failed"))
        .unwrap();
    db.update_task_status(&task, TaskStatus::Blocked).unwrap();
    let worker = CountingWorker::new();

    assert!(
        agent::dispatch_with_worker_on_db(
            &task,
            &worker,
            &db,
            &repo,
            "eligible-codex",
            &FakeValidationRunner::success()
        )
        .is_err()
    );
    orc::app::OrcApp::open(&db_path, &repo)
        .unwrap()
        .requeue(&task)
        .unwrap();
    agent::dispatch_with_worker_on_db(
        &task,
        &worker,
        &db,
        &repo,
        "eligible-codex",
        &FakeValidationRunner::success(),
    )
    .unwrap();

    assert_eq!(worker.calls.load(Ordering::SeqCst), 1);
    assert_eq!(db.list_agent_runs_for_task(&task).unwrap().len(), 2);
}
