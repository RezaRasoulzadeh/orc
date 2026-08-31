use anyhow::Result;
use orc::agent;
use orc::app::OrcApp;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides};
use orc::registry::{self, AgentAction, AgentDefinition};
use orc::storage::Database;
use orc::task::TaskStatus;
use orc::validation::test_helpers::FakeValidationRunner;
use orc::worker::test_helpers::{FailingSpawnWorker, FakeWorker};
use orc::worker::{Worker, WorkerExecution, WorkerOutcome};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use tempfile::TempDir;

struct ValidationLifecycleReviewBackend;

struct EvolvingWorker;
impl Worker for EvolvingWorker {
    fn execute(&self, _: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        let path = cwd.join("feature.txt");
        let content = "revised\n";
        std::fs::write(path, content).map_err(|error| error.to_string())?;
        Ok((WorkerOutcome::Success, Some(r#"{"claims":[{"blocker_id":"BLK-validation","status":"addressed","implementation_summary":"fixed validation","changed_files":["lifecycle-change.txt"],"evidence":[{"changed_file":"lifecycle-change.txt","validation_command":"validation","test_names":["validation"]}],"validation_evidence":"validation passes","unresolved_risk":null}]}"#.into())))
    }
}

struct PassIgnoringValidationReviewBackend;

impl ActionBackend for PassIgnoringValidationReviewBackend {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<registry::ReasoningEffort>,
    ) -> Result<ActionExecution> {
        assert_eq!(action, AgentAction::Review);
        // The reviewer claims a clean PASS despite the task-specific
        // validation Orc executed having failed; automated review must
        // still surface a blocker and not accept this as PASS.
        Ok(ActionExecution {
            output: r#"{"verdict":"PASS","findings":[],"blocking_findings":[],"blockers":[]}"#
                .into(),
            token_usage: None,
        })
    }
}

impl ActionBackend for ValidationLifecycleReviewBackend {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        _: &str,
        _: Option<&str>,
        _: Option<registry::ReasoningEffort>,
    ) -> Result<ActionExecution> {
        assert_eq!(action, AgentAction::Review);
        Ok(ActionExecution {
            output: r#"{"verdict":"REVISE","findings":["validation failed"],"blocking_findings":["validation failed"],"revision_feedback":"Fix the validation failure","blockers":[{"id":"BLK-validation","blocker_key":"validation","requirement_ref":"validation","evidence":"validation failed","severity":"high","acceptance_condition":"validation passes","status":"unresolved","finding":"validation failed"}]}"#.into(),
            token_usage: None,
        })
    }
}

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
        Ok((
            WorkerOutcome::Success,
            Some(
                "executed\nOPERATION PERFORMED: modify\nVERIFICATION PASSED: configured validation evidence"
                    .into(),
            ),
        ))
    }

    fn execute_planned_step(
        &self,
        step: &orc::worker_protocol::PlannedStep,
        _: &str,
        cwd: &Path,
        _: &str,
        _: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(cwd.join(&step.intent), "dispatched\n")
            .map_err(|error| error.to_string())?;
        Ok(WorkerExecution {
            outcome: WorkerOutcome::Success,
            output: Some(format!(
                "OPERATION PERFORMED: {}\nVERIFICATION PASSED: configured validation evidence\n",
                orc::worker_protocol::operation_name(&step.operations[0])
            )),
            token_usage: None,
        })
    }

    fn execute_structured_with_progress_and_usage(
        &self,
        prompt: &str,
        cwd: &Path,
        _: &str,
        _: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(cwd.join("eligibility-change.txt"), "dispatched\n")
            .map_err(|e| e.to_string())?;
        let packet_json = prompt
            .split("## Authoritative Orc packet")
            .nth(1)
            .and_then(|value| value.find('{').map(|i| &value[i..]))
            .ok_or("missing plan")?;
        let packet: serde_json::Value =
            serde_json::from_str(packet_json).map_err(|error| error.to_string())?;
        let plan: orc::worker_protocol::WorkerPlan =
            serde_json::from_value(packet["execution_plan"].clone())
                .map_err(|error| error.to_string())?;
        let step = &plan.steps[0];
        Ok(WorkerExecution {
            outcome: WorkerOutcome::Success,
            token_usage: None,
            output: Some(
                serde_json::json!({"step_results":[{"step_id":step.id,
                "operations_performed":step.operations,"affected_files":["eligibility-change.txt"],
                "observed":["checkpoint completed"],"verification_passed":[]}],"summary":"done"})
                .to_string(),
            ),
        })
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

    let db_path = repo_dir.join(".orc/orc.db");
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

    let db_path = repo_dir.join(".orc/orc.db");
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
fn dispatch_validation_evidence_is_reused_by_semantic_review() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);
    std::fs::write(
        repo_dir.join(".orc/validation.toml"),
        "commands = [\"cargo test validation-lifecycle\"]\n",
    )
    .unwrap();
    Command::new("git")
        .current_dir(&repo_dir)
        .args(["add", ".orc/validation.toml"])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&repo_dir)
        .args(["commit", "-m", "configure failing validation test"])
        .output()
        .unwrap();

    let db_path = repo_dir.join("orc.db");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("validation-lifecycle").unwrap();
    register_eligible_agent(&db);
    db.insert_agent(&AgentDefinition {
        id: "eligible-reviewer".into(),
        backend: "codex".into(),
        execution_mode: registry::AUTOMATED.into(),
        display_name: "Reviewer".into(),
        enabled: true,
        priority: 100,
        capabilities: vec!["review".into()],
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
        actions: vec![AgentAction::Review],
    })
    .unwrap();
    let task = get_unique_task_id();
    db.insert_task_with_id(
        project,
        &task,
        "Validation lifecycle",
        "preserve the worktree",
        "developer",
        orc::task::TaskPriority::Normal,
    )
    .unwrap();

    // Dispatch owns deterministic validation and publishes fresh evidence
    // before making the task Review-ready.
    let worker = FakeWorker::new_success(None);
    let result = agent::dispatch_with_worker_on_db(
        &task,
        &worker,
        &db,
        &repo_dir,
        "eligible-codex",
        &FakeValidationRunner::success(),
    );
    assert!(result.is_ok());
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Review
    );

    let runs = db.list_agent_runs_for_task(&task).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, "completed");
    assert!(
        db.latest_validation_result_for_run(runs[0].id)
            .unwrap()
            .is_some(),
        "dispatch must persist current validation evidence"
    );

    // Review consumes the existing pass and stays semantic-only.
    let app = OrcApp::open(&db_path, &repo_dir).unwrap();
    let overrides = ActionOverrides {
        agent_id: Some("eligible-reviewer".into()),
        ..ActionOverrides::default()
    };
    let (review_run, review) = app
        .automated_review_with_backend(
            &task,
            &overrides,
            &PassIgnoringValidationReviewBackend,
            &FakeValidationRunner::failing_on("cargo test validation-lifecycle"),
        )
        .unwrap();
    assert_eq!(review.verdict, "PASS");
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::AcceptanceReady
    );
    assert!(review.blockers.is_empty());
    assert!(
        db.latest_validation_result_for_run(review_run)
            .unwrap()
            .is_none()
    );
    assert!(repo_dir.join(".orc/worktrees").join(&task).exists());

    let requeue = app.requeue(&task).unwrap_err().to_string();
    assert!(requeue.contains("cannot be requeued") || requeue.contains("not active"));
    assert_eq!(
        app.task(&task).unwrap().unwrap().status,
        TaskStatus::AcceptanceReady
    );
}

#[test]
fn semantic_review_revise_validate_review_accept_is_one_production_lifecycle() {
    let dir = TempDir::new().unwrap();
    let repo_dir = dir.path().join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    init_temp_git_repo(&repo_dir);
    std::fs::write(
        repo_dir.join(".orc/validation.toml"),
        "commands = [\"validation\"]\n",
    )
    .unwrap();
    Command::new("git")
        .current_dir(&repo_dir)
        .args(["add", ".orc/validation.toml"])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(&repo_dir)
        .args(["commit", "-m", "configure validation"])
        .output()
        .unwrap();
    let db_path = repo_dir.join(".orc/orc.db");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("validation-e2e").unwrap();
    let mut discovery = orc::discovery::build_snapshot(&repo_dir).unwrap();
    discovery.fingerprint = "manual-lifecycle".into();
    db.store_discovery_snapshot(project, &discovery).unwrap();
    register_eligible_agent(&db);
    db.insert_agent(&AgentDefinition {
        id: "eligible-reviewer".into(),
        backend: "codex".into(),
        execution_mode: registry::AUTOMATED.into(),
        display_name: "Reviewer".into(),
        enabled: true,
        priority: 100,
        capabilities: vec!["review".into()],
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
        actions: vec![AgentAction::Review],
    })
    .unwrap();
    let task = get_unique_task_id();
    db.insert_task_with_id(
        project,
        &task,
        "Validation e2e",
        "fix it",
        "developer",
        orc::task::TaskPriority::Normal,
    )
    .unwrap();

    // This fixture has no selected validation commands, so Dispatch succeeds
    // without invoking the ValidationRunner.
    assert!(
        agent::dispatch_with_worker_on_db(
            &task,
            &FakeWorker::new_success(None),
            &db,
            &repo_dir,
            "eligible-codex",
            &FakeValidationRunner::success(),
        )
        .is_ok()
    );
    let worker = EvolvingWorker;
    let worktree = repo_dir.join(db.get_worktree_metadata(&task).unwrap().unwrap().1);
    assert!(worktree.exists());
    let app = OrcApp::open(&db_path, &repo_dir).unwrap();
    assert!(app.requeue(&task).is_err());

    let overrides = ActionOverrides {
        agent_id: Some("eligible-reviewer".into()),
        ..ActionOverrides::default()
    };
    let (review_run, review) = app
        .automated_review_with_backend(
            &task,
            &overrides,
            &ValidationLifecycleReviewBackend,
            &FakeValidationRunner::success(),
        )
        .unwrap();
    assert_eq!(review.verdict, "REVISE");
    assert!(db.actionable_revision_contract(&task).unwrap().is_some());
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::RevisionRequired
    );

    agent::revise_with_worker_on_db(
        &task,
        "",
        &worker,
        &db,
        &repo_dir,
        "eligible-codex",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Review
    );
    assert!(worktree.exists());
    assert!(db.actionable_revision_contract(&task).unwrap().is_none());

    let (_, pass) = app
        .automated_review_with_backend(
            &task,
            &overrides,
            &PassIgnoringValidationReviewBackend,
            &FakeValidationRunner::success(),
        )
        .unwrap();
    assert_eq!(pass.verdict, "PASS");
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::AcceptanceReady
    );

    let accepted = agent::accept_task(&db, &task, &repo_dir);
    assert!(
        accepted.is_ok(),
        "PASS acceptance should succeed: {accepted:?}"
    );
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Done
    );
    assert!(
        db.load_discovery_snapshot(project, "manual-lifecycle")
            .unwrap()
            .is_some()
    );
    assert!(review_run > 0);
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
    assert_eq!(run.agent, "eligible-codex");
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
    let mut proposal = orc::protocol::TaskProposal {
        local_id: task.clone(),
        title: "task".into(),
        objective: "objective".into(),
        role: "developer".into(),
        priority: orc::task::TaskPriority::Normal,
        depends_on: vec![],
        capabilities: vec!["code".into(), "terminal".into()],
        scope_mode: None,
        context_files: vec![],
        expected_changes: vec!["eligibility-change.txt".into()],
        unchanged: vec!["unrelated behavior".into()],
        acceptance_criteria: vec!["the change is written".into()],
        required_tests: vec!["configured validation pipeline".into()],
        validation: vec!["configured validation evidence".into()],
        execution_hints: Default::default(),
        risk_factors: vec![],
    };
    proposal.execution_hints.effort = Some("medium".into());
    proposal.execution_hints.effort_reason = Some("moderate verification burden".into());
    db.set_task_proposal_metadata(&task, &proposal).unwrap();
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
        db.list_agent_runs_for_task(&task)
            .unwrap()
            .pop()
            .unwrap()
            .resolved_reasoning_effort,
        Some(orc::registry::ReasoningEffort::Medium)
    );
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
