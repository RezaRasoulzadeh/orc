use anyhow::Result;
use orc::agent::{accept_task, dispatch_with_worker_and_db_as_with_runner, reject_task};
use orc::git;
use orc::review;
use orc::storage::Database;
use orc::task::{TaskPriority, TaskStatus};
use orc::validation::test_helpers::FakeValidationRunner;
use orc::validation::{
    ValidationCategory, ValidationFailureClassification, ValidationRunner, ValidationStepResult,
};
use orc::worker::{Worker, WorkerOutcome};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use tempfile::TempDir;

struct WritingWorker;
impl Worker for WritingWorker {
    fn execute(&self, _: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        let file = cwd.join("feature.txt");
        let content = if file.exists() {
            "implemented again\n"
        } else {
            "implemented\n"
        };
        std::fs::write(file, content).map_err(|e| e.to_string())?;
        Ok((WorkerOutcome::Success, Some("full worker output".into())))
    }
}
struct NoChangeWorker;
impl Worker for NoChangeWorker {
    fn execute(&self, _: &str, _: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        Ok((WorkerOutcome::Success, None))
    }
}

struct ConflictingWorker;
impl Worker for ConflictingWorker {
    fn execute(&self, _: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        std::fs::write(cwd.join("README.md"), "task version\n").map_err(|e| e.to_string())?;
        Ok((WorkerOutcome::Success, Some("changed README".into())))
    }
}

struct RepairWorker {
    calls: Mutex<Vec<(String, std::path::PathBuf)>>,
}

impl RepairWorker {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl Worker for RepairWorker {
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        let mut calls = self.calls.lock().unwrap();
        calls.push((prompt.to_owned(), cwd.to_owned()));
        let content = if calls.len() == 1 {
            "needs repair\n"
        } else {
            "repaired\n"
        };
        std::fs::write(cwd.join("feature.txt"), content).map_err(|error| error.to_string())?;
        Ok((
            WorkerOutcome::Success,
            Some(format!("worker call {}", calls.len())),
        ))
    }
}

struct SequenceValidationRunner {
    results: Mutex<VecDeque<ValidationStepResult>>,
    directories: Mutex<Vec<std::path::PathBuf>>,
}

impl SequenceValidationRunner {
    fn new(results: Vec<ValidationStepResult>) -> Self {
        Self {
            results: Mutex::new(results.into()),
            directories: Mutex::new(Vec::new()),
        }
    }
}

impl ValidationRunner for SequenceValidationRunner {
    fn run(&self, _: &str, working_dir: &Path) -> anyhow::Result<ValidationStepResult> {
        self.directories
            .lock()
            .unwrap()
            .push(working_dir.to_owned());
        Ok(self.results.lock().unwrap().pop_front().unwrap())
    }
}

fn validation_result(category: ValidationCategory, diagnostics: &str) -> ValidationStepResult {
    let passed = category == ValidationCategory::Success;
    ValidationStepResult {
        command: "check".to_owned(),
        category,
        passed,
        stdout: if passed { "ok" } else { "partial output" }.to_owned(),
        stderr: if passed { "" } else { "exact stderr" }.to_owned(),
        exit_status: Some(if passed { 0 } else { 1 }),
        diagnostics: (!diagnostics.is_empty()).then(|| diagnostics.to_owned()),
        failure_classification: (!passed).then_some(
            if matches!(
                category,
                ValidationCategory::Timeout | ValidationCategory::Infrastructure
            ) {
                ValidationFailureClassification::Infrastructure
            } else {
                ValidationFailureClassification::Implementation
            },
        ),
        fallback_command: None,
    }
}

fn cmd(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn setup() -> (TempDir, Database, String) {
    let dir = tempfile::tempdir().unwrap();
    cmd(dir.path(), &["init"]);
    cmd(dir.path(), &["config", "user.email", "test@example.com"]);
    cmd(dir.path(), &["config", "user.name", "Test"]);
    std::fs::create_dir_all(dir.path().join(".orc")).unwrap();
    std::fs::write(dir.path().join(".orc/engineering.md"), "# Contract\n").unwrap();
    std::fs::write(
        dir.path().join(".orc/validation.toml"),
        "commands = [\"check\"]\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("README.md"), "base\n").unwrap();
    cmd(dir.path(), &["add", "."]);
    cmd(dir.path(), &["commit", "-m", "base"]);
    let db_path = dir.path().join(".orc/orc.db");
    let db = Database::init(&db_path).unwrap();
    let project = db.create_project("test").unwrap();
    let task = db
        .insert_task(
            project,
            "Dispatch review",
            "change a file",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    (dir, db, task)
}

#[test]
fn dispatch_summary_and_review_show_real_task_worktree_changes() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    let summary = dispatch_with_worker_and_db_as_with_runner(
        &task,
        &WritingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    assert!(review::format_dispatch(&summary).contains("Worktree"));
    assert!(review::format_dispatch(&summary).contains("Run"));
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Review
    );
    std::fs::write(dir.path().join("main-only.txt"), "main\n").unwrap();
    let diff = git::show_diff(&task, dir.path()).unwrap();
    assert!(diff.contains("feature.txt"));
    assert!(!diff.contains("main-only.txt"));
    let view = review::build_review(&db, &task, dir.path()).unwrap();
    assert!(!review::format_review(&view).contains("full worker output"));
    assert!(review::format_review(&view).contains("feature.txt"));
    assert!(!review::format_review(&view).contains("\nDiff\n"));
    assert!(review::format_review_with_diff(&view, Some(&view.changes.diff)).contains("\nDiff\n"));
    assert!(
        review::format_review_file(&view, "feature.txt")
            .unwrap()
            .contains("feature.txt")
    );
}

#[test]
fn untracked_and_runtime_db_artifacts_are_handled_correctly() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    dispatch_with_worker_and_db_as_with_runner(
        &task,
        &WritingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    let (_, path) = db.get_worktree_metadata(&task).unwrap().unwrap();
    let worktree = dir.path().join(path);
    std::fs::write(worktree.join("untracked.txt"), "untracked\n").unwrap();
    std::fs::create_dir_all(worktree.join(".orc")).unwrap();
    std::fs::write(worktree.join(".orc/orc.db"), "runtime").unwrap();
    let changes = git::inspect_worktree(&worktree, dir.path()).unwrap();
    assert!(changes.diff.contains("untracked.txt"));
    assert!(!changes.diff.contains(".orc/orc.db"));
    assert!(!changes.files.iter().any(|f| f.path == ".orc/orc.db"));
}

#[test]
fn no_change_and_validation_failure_block_task() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    assert!(
        dispatch_with_worker_and_db_as_with_runner(
            &task,
            &NoChangeWorker,
            db_path.to_str().unwrap(),
            dir.path(),
            "fake",
            &FakeValidationRunner::success()
        )
        .is_err()
    );
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Blocked
    );

    let task2 = db
        .insert_task(
            db.get_project_id().unwrap().unwrap(),
            "Fail validation",
            "change",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    assert!(
        dispatch_with_worker_and_db_as_with_runner(
            &task2,
            &WritingWorker,
            db_path.to_str().unwrap(),
            dir.path(),
            "fake",
            &FakeValidationRunner::failing_on("check")
        )
        .is_err()
    );
    assert_eq!(
        db.get_task(&task2).unwrap().unwrap().status,
        TaskStatus::Blocked
    );
    let run_id = db.list_agent_runs_for_task(&task2).unwrap()[0].id;
    let validation = db
        .list_lifecycle_events_for_run(run_id, 20)
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "validation_result")
        .unwrap();
    assert!(validation.payload.unwrap().contains("\"passed\":false"));
}

#[test]
fn validation_failure_repairs_in_same_worktree_and_persists_diagnostics() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    let worker = RepairWorker::new();
    let runner = SequenceValidationRunner::new(vec![
        validation_result(ValidationCategory::Test, "test assertion failed"),
        validation_result(ValidationCategory::Success, ""),
    ]);
    let summary = dispatch_with_worker_and_db_as_with_runner(
        &task,
        &worker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &runner,
    )
    .unwrap();
    let calls = worker.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1, calls[1].1);
    assert!(calls[1].0.contains("test assertion failed"));
    assert!(calls[1].0.contains("exact stderr"));
    assert!(calls[1].0.contains("Preserve valid existing work"));
    assert_eq!(
        std::fs::read_to_string(calls[1].1.join("feature.txt")).unwrap(),
        "repaired\n"
    );
    assert_eq!(summary.run_status, "completed");
    let events = db
        .list_lifecycle_events_for_run(summary.run_id, 30)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "validation_attempt")
            .count(),
        2
    );
    assert!(events.iter().any(|event| {
        event.kind == "validation_result"
            && event.payload.as_deref().is_some_and(|payload| {
                payload.contains("test assertion failed") && payload.contains("exact stderr")
            })
    }));
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "validation_repair_started")
            .count(),
        1
    );
}

#[test]
fn validation_repair_is_bounded_and_infrastructure_only_retries_validation() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    let worker = RepairWorker::new();
    let runner = SequenceValidationRunner::new(
        (0..4)
            .map(|_| validation_result(ValidationCategory::Lint, "lint failed"))
            .collect(),
    );
    assert!(
        dispatch_with_worker_and_db_as_with_runner(
            &task,
            &worker,
            db_path.to_str().unwrap(),
            dir.path(),
            "fake",
            &runner,
        )
        .is_err()
    );
    assert_eq!(worker.calls.lock().unwrap().len(), 4);
    let run_id = db.list_agent_runs_for_task(&task).unwrap()[0].id;
    let events = db.list_lifecycle_events_for_run(run_id, 40).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "validation_repair_completed")
            .count(),
        3
    );

    let task = db
        .insert_task(
            db.get_project_id().unwrap().unwrap(),
            "Infrastructure validation",
            "change",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    let worker = RepairWorker::new();
    let runner = SequenceValidationRunner::new(vec![
        validation_result(ValidationCategory::Infrastructure, "registry unavailable"),
        validation_result(ValidationCategory::Infrastructure, "registry unavailable"),
        validation_result(ValidationCategory::Success, ""),
    ]);
    let summary = dispatch_with_worker_and_db_as_with_runner(
        &task,
        &worker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &runner,
    )
    .unwrap();
    assert_eq!(worker.calls.lock().unwrap().len(), 1);
    let events = db
        .list_lifecycle_events_for_run(summary.run_id, 30)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "validation_attempt")
            .count(),
        3
    );
    assert!(
        !events
            .iter()
            .any(|event| event.kind == "validation_repair_started")
    );
}

#[test]
fn accept_integrates_and_reject_preserves_worktree() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    dispatch_with_worker_and_db_as_with_runner(
        &task,
        &WritingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    accept_task(&db, &task, dir.path()).unwrap();
    assert_eq!(
        db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::Done
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("feature.txt")).unwrap(),
        "implemented\n"
    );

    let task2 = db
        .insert_task(
            db.get_project_id().unwrap().unwrap(),
            "Reject",
            "change",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    dispatch_with_worker_and_db_as_with_runner(
        &task2,
        &WritingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    let (_, path) = db.get_worktree_metadata(&task2).unwrap().unwrap();
    reject_task(&db, &task2, Some("needs revision")).unwrap();
    assert_eq!(
        db.get_task(&task2).unwrap().unwrap().status,
        TaskStatus::Ready
    );
    assert!(dir.path().join(path).exists());
    dispatch_with_worker_and_db_as_with_runner(
        &task2,
        &WritingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    assert_eq!(
        db.get_task(&task2).unwrap().unwrap().status,
        TaskStatus::Review
    );
}

#[test]
fn accept_merges_diverged_non_conflicting_main_and_aborts_conflicts_safely() {
    let (dir, db, task) = setup();
    let db_path = dir.path().join(".orc/orc.db");
    dispatch_with_worker_and_db_as_with_runner(
        &task,
        &WritingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    std::fs::write(dir.path().join("main-only.txt"), "main\n").unwrap();
    cmd(dir.path(), &["add", "main-only.txt"]);
    cmd(dir.path(), &["commit", "-m", "main changes"]);
    accept_task(&db, &task, dir.path()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("feature.txt")).unwrap(),
        "implemented\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("main-only.txt")).unwrap(),
        "main\n"
    );

    let conflicting_task = db
        .insert_task(
            db.get_project_id().unwrap().unwrap(),
            "Conflict",
            "change README",
            "developer",
            TaskPriority::Normal,
        )
        .unwrap();
    dispatch_with_worker_and_db_as_with_runner(
        &conflicting_task,
        &ConflictingWorker,
        db_path.to_str().unwrap(),
        dir.path(),
        "fake",
        &FakeValidationRunner::success(),
    )
    .unwrap();
    std::fs::write(dir.path().join("README.md"), "main version\n").unwrap();
    cmd(dir.path(), &["add", "README.md"]);
    cmd(dir.path(), &["commit", "-m", "conflicting main changes"]);
    let (_, path) = db
        .get_worktree_metadata(&conflicting_task)
        .unwrap()
        .unwrap();
    let error = accept_task(&db, &conflicting_task, dir.path()).unwrap_err();
    assert!(error.to_string().contains("conflicts"));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("README.md")).unwrap(),
        "main version\n"
    );
    assert!(dir.path().join(path).exists());
    assert_eq!(
        db.get_task(&conflicting_task).unwrap().unwrap().status,
        TaskStatus::Review
    );
}

#[test]
fn stale_task_branch_is_not_reused_after_worktree_disappears() {
    let (dir, _db, task) = setup();
    let old_worktree = git::ensure_worktree(&task, dir.path()).unwrap();
    let old_commit = git_output(dir.path(), &["rev-parse", &old_worktree.0]);

    std::fs::write(dir.path().join("main.txt"), "advanced\n").unwrap();
    cmd(dir.path(), &["add", "main.txt"]);
    cmd(dir.path(), &["commit", "-m", "advance main"]);
    let new_commit = git_output(dir.path(), &["rev-parse", "HEAD"]);
    git::remove_worktree(dir.path(), &old_worktree.1).unwrap();

    let (_, new_path) = git::ensure_worktree(&task, dir.path()).unwrap();
    let prepared_commit = git_output(dir.path().join(new_path).as_path(), &["rev-parse", "HEAD"]);

    assert_eq!(prepared_commit, new_commit);
    assert_ne!(prepared_commit, old_commit);
}

fn git_output(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {:?}", args);
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
