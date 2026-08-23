use orc::agent::{self, submit_patch_with_runner, submit_run};
use orc::registry::{AVAILABLE, AgentDefinition, MANUAL};
use orc::storage::Database;
use orc::task::{TaskPriority, TaskStatus};
use orc::validation::test_helpers::FakeValidationRunner;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn manual_agent() -> AgentDefinition {
    AgentDefinition {
        id: "manual-coder".into(),
        backend: "claude".into(),
        execution_mode: MANUAL.into(),
        display_name: "Manual Coder".into(),
        enabled: true,
        priority: 100,
        capabilities: vec!["code".into(), "terminal".into()],
        status: AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: None,
        reasoning_effort: None,
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions: vec![orc::registry::AgentAction::Code],
    }
}

fn init_git_repo(repo_path: &Path) {
    Command::new("git")
        .current_dir(repo_path)
        .arg("init")
        .arg(".")
        .output()
        .expect("git init");

    Command::new("git")
        .current_dir(repo_path)
        .arg("config")
        .arg("user.email")
        .arg("test@example.com")
        .output()
        .expect("git config email");

    Command::new("git")
        .current_dir(repo_path)
        .arg("config")
        .arg("user.name")
        .arg("Test User")
        .output()
        .expect("git config name");

    let readme = repo_path.join("README.md");
    std::fs::write(&readme, "initial line 1\ninitial line 2\n").expect("write readme");

    Command::new("git")
        .current_dir(repo_path)
        .arg("add")
        .arg(".")
        .output()
        .expect("git add");

    Command::new("git")
        .current_dir(repo_path)
        .arg("commit")
        .arg("-m")
        .arg("initial commit")
        .output()
        .expect("git commit");
}

fn setup_test_env() -> (TempDir, Database, String) {
    let dir = TempDir::new().unwrap();
    init_git_repo(dir.path());

    let orc_dir = dir.path().join(".orc");
    std::fs::create_dir_all(&orc_dir).unwrap();
    std::fs::write(
        orc_dir.join("engineering.md"),
        "# Contract\n\n## Tests and validation\nEvery implementation must pass:\n\ncargo test\n",
    )
    .unwrap();

    let db_path = orc_dir.join("orc.db");
    let db = Database::init(&db_path).unwrap();
    let project_id = db.create_project("test-project").unwrap();
    let task_id = db
        .insert_task(
            project_id,
            "Add feature",
            "Implement new feature",
            "coder",
            TaskPriority::Normal,
        )
        .unwrap();

    db.insert_agent(&manual_agent()).unwrap();

    (dir, db, task_id)
}

#[test]
fn test_manual_waiting_run_accepts_patch() {
    let (dir, db, task_id) = setup_test_env();
    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run = db.list_agent_runs_for_task(&task_id).unwrap()[0].clone();
    assert_eq!(run.status, "waiting_external");
    assert_eq!(run.execution_mode, MANUAL);

    let patch = "diff --git a/feature.txt b/feature.txt
new file mode 100644
--- /dev/null
+++ b/feature.txt
@@ -0,0 +1 @@
+feature implementation
";
    let runner = FakeValidationRunner::success();
    let outcome =
        submit_patch_with_runner(&db, run.id, patch, dir.path(), &runner).expect("submit patch");

    assert_eq!(outcome.run_id, run.id);
    assert_eq!(outcome.task_id, task_id);
    assert!(outcome.validation_report.is_success());

    let updated_run = db.get_agent_run(run.id).unwrap().unwrap();
    assert_eq!(updated_run.status, "completed");
    assert!(
        updated_run
            .output
            .unwrap()
            .contains("feature implementation")
    );

    let updated_task = db.get_task(&task_id).unwrap().unwrap();
    assert_eq!(updated_task.status, TaskStatus::Review);
}

#[test]
fn test_automated_run_rejects_submit_patch() {
    let (dir, db, task_id) = setup_test_env();
    let project_id = db.get_project_id().unwrap().unwrap();

    // Create an automated run
    let run_id = db
        .create_agent_run_with_mode(project_id, &task_id, "auto-agent", "automated")
        .unwrap();

    let patch = "diff --git a/test.txt b/test.txt
new file mode 100644
--- /dev/null
+++ b/test.txt
@@ -0,0 +1 @@
+test
";
    let runner = FakeValidationRunner::success();
    let err = submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).unwrap_err();
    assert!(
        err.to_string()
            .contains("only manual runs accept submit-patch")
    );

    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_ne!(task.status, TaskStatus::Review);
}

#[test]
fn test_completed_and_failed_manual_run_rejects_resubmission() {
    let (dir, db, task_id) = setup_test_env();
    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let patch = "diff --git a/file.txt b/file.txt
new file mode 100644
--- /dev/null
+++ b/file.txt
@@ -0,0 +1 @@
+content
";
    let runner = FakeValidationRunner::success();
    submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).unwrap();

    // Resubmitting to already completed run must fail
    let err = submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).unwrap_err();
    assert!(
        err.to_string()
            .contains("only waiting_external manual runs accept submit-patch")
    );

    // Create a new task and failed run
    let project_id = db.get_project_id().unwrap().unwrap();
    let task_id2 = db
        .insert_task(
            project_id,
            "Second task",
            "Objective",
            "coder",
            TaskPriority::Normal,
        )
        .unwrap();
    agent::dispatch_manual(&task_id2, &manual_agent(), &db, dir.path()).unwrap();
    let run_id2 = db.list_agent_runs_for_task(&task_id2).unwrap()[0].id;
    agent::fail_run(&db, run_id2, "manual failure").unwrap();

    // Resubmitting to failed run must fail
    let err2 = submit_patch_with_runner(&db, run_id2, patch, dir.path(), &runner).unwrap_err();
    assert!(
        err2.to_string()
            .contains("only waiting_external manual runs accept submit-patch")
    );
}

#[test]
fn test_malformed_patch_rejected() {
    let (dir, db, task_id) = setup_test_env();
    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let runner = FakeValidationRunner::success();

    // Empty patch
    let err_empty = submit_patch_with_runner(&db, run_id, "", dir.path(), &runner).unwrap_err();
    assert!(err_empty.to_string().contains("patch content is empty"));

    // Whitespace only
    let err_ws =
        submit_patch_with_runner(&db, run_id, "   \n\t  ", dir.path(), &runner).unwrap_err();
    assert!(err_ws.to_string().contains("patch content is empty"));

    // Malformed diff text
    let err_garbage =
        submit_patch_with_runner(&db, run_id, "This is not a git diff", dir.path(), &runner)
            .unwrap_err();
    assert!(err_garbage.to_string().contains("patch validation failed"));

    // Task is not review
    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_ne!(task.status, TaskStatus::Review);
}

#[test]
fn test_patch_validation_failure_does_not_modify_worktree() {
    let (dir, db, task_id) = setup_test_env();
    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    // Conflicting patch targeting wrong base content
    let conflict_patch = "diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1,2 +1,2 @@
-nonexistent line
+modified line
";
    let runner = FakeValidationRunner::success();
    let err =
        submit_patch_with_runner(&db, run_id, conflict_patch, dir.path(), &runner).unwrap_err();
    assert!(err.to_string().contains("patch validation failed"));

    // Verify task worktree exists and README.md was NOT modified
    let worktree_dir = dir.path().join(".orc/worktrees").join(&task_id);
    assert!(worktree_dir.exists());
    let readme_content = std::fs::read_to_string(worktree_dir.join("README.md")).unwrap();
    assert_eq!(readme_content, "initial line 1\ninitial line 2\n");

    // Run remains waiting_external (actionable)
    let run = db.get_agent_run(run_id).unwrap().unwrap();
    assert_eq!(run.status, "waiting_external");
}

#[test]
fn test_valid_patch_applies_only_in_task_worktree_and_main_checkout_unchanged() {
    let (dir, db, task_id) = setup_test_env();
    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let patch = "diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1,2 +1,3 @@
 initial line 1
+inserted line
 initial line 2
diff --git a/new_module.rs b/new_module.rs
new file mode 100644
--- /dev/null
+++ b/new_module.rs
@@ -0,0 +1 @@
+pub fn test() {}
";
    let runner = FakeValidationRunner::success();
    let outcome =
        submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).expect("submit patch");

    let worktree_dir = dir.path().join(&outcome.worktree_path);

    // Worktree has applied modifications
    let worktree_readme = std::fs::read_to_string(worktree_dir.join("README.md")).unwrap();
    assert_eq!(
        worktree_readme,
        "initial line 1\ninserted line\ninitial line 2\n"
    );
    assert!(worktree_dir.join("new_module.rs").exists());

    // Main checkout remains untouched
    let main_readme = std::fs::read_to_string(dir.path().join("README.md")).unwrap();
    assert_eq!(main_readme, "initial line 1\ninitial line 2\n");
    assert!(!dir.path().join("new_module.rs").exists());
}

#[test]
fn test_existing_worktree_is_reused() {
    let (dir, db, task_id) = setup_test_env();

    // Create worktree before submission
    let (_branch, worktree_path) = orc::git::create_worktree(&task_id, dir.path()).unwrap();
    let worktree_dir = dir.path().join(&worktree_path);
    let marker_file = worktree_dir.join("marker.txt");
    std::fs::write(&marker_file, "reuse-me").unwrap();

    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let patch = "diff --git a/other.txt b/other.txt
new file mode 100644
--- /dev/null
+++ b/other.txt
@@ -0,0 +1 @@
+other
";
    let runner = FakeValidationRunner::success();
    let outcome =
        submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).expect("submit patch");

    assert_eq!(outcome.worktree_path, worktree_path);
    assert!(
        marker_file.exists(),
        "marker file should still exist in reused worktree"
    );
    assert_eq!(std::fs::read_to_string(&marker_file).unwrap(), "reuse-me");
}

#[test]
fn test_validation_commands_run_after_apply() {
    let (dir, db, task_id) = setup_test_env();

    // Create custom .orc/validation.toml
    std::fs::write(
        dir.path().join(".orc/validation.toml"),
        r#"commands = ["cargo fmt --check", "cargo test --lib"]"#,
    )
    .unwrap();

    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let patch = "diff --git a/foo.txt b/foo.txt
new file mode 100644
--- /dev/null
+++ b/foo.txt
@@ -0,0 +1 @@
+foo
";
    let runner = FakeValidationRunner::success();
    let outcome =
        submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).expect("submit patch");

    assert_eq!(outcome.validation_report.steps.len(), 2);
    assert_eq!(
        runner.executed_commands(),
        vec!["cargo fmt --check", "cargo test --lib"]
    );
}

#[test]
fn test_failed_validation_lifecycle_and_worktree_preserved() {
    let (dir, db, task_id) = setup_test_env();

    std::fs::write(
        dir.path().join(".orc/validation.toml"),
        r#"commands = ["cargo test"]"#,
    )
    .unwrap();

    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let patch = "diff --git a/applied.txt b/applied.txt
new file mode 100644
--- /dev/null
+++ b/applied.txt
@@ -0,0 +1 @@
+applied content
";
    // Runner fails on cargo test
    let runner = FakeValidationRunner::failing_on("cargo test");
    let err = submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).unwrap_err();
    assert!(err.to_string().contains("Validation failed"));

    // Run status must be failed
    let run = db.get_agent_run(run_id).unwrap().unwrap();
    assert_eq!(run.status, "failed");
    assert!(run.output.unwrap().contains("Validation:\n  cargo test"));
    let events = db.list_lifecycle_events_for_run(run_id, 20).unwrap();
    let validation = events
        .iter()
        .find(|event| event.kind == "validation_result")
        .unwrap();
    assert!(validation.payload.as_deref().unwrap().contains("steps"));
    let changes = events
        .iter()
        .find(|event| event.kind == "change_evidence")
        .unwrap();
    assert!(changes.payload.as_deref().unwrap().contains("applied.txt"));

    // Task status must be blocked (not review)
    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Blocked);

    // Applied worktree must be preserved for debugging
    let worktree_file = dir
        .path()
        .join(".orc/worktrees")
        .join(&task_id)
        .join("applied.txt");
    assert!(
        worktree_file.exists(),
        "applied patch files must be preserved in worktree after validation failure"
    );
    assert_eq!(
        std::fs::read_to_string(worktree_file).unwrap(),
        "applied content\n"
    );
}

#[test]
fn test_patch_output_persists_and_reopening_db() {
    let (dir, db, task_id) = setup_test_env();
    let db_path = dir.path().join(".orc/orc.db");

    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let patch = "diff --git a/persisted.txt b/persisted.txt
new file mode 100644
--- /dev/null
+++ b/persisted.txt
@@ -0,0 +1 @@
+persisted data
";
    let runner = FakeValidationRunner::success();
    submit_patch_with_runner(&db, run_id, patch, dir.path(), &runner).unwrap();

    drop(db);

    // Reopen DB
    let reopened = Database::open(&db_path).unwrap();
    let task = reopened.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);

    let run = reopened.get_agent_run(run_id).unwrap().unwrap();
    assert_eq!(run.status, "completed");
    assert!(run.output.as_ref().unwrap().contains("persisted data"));
    assert!(run.output.as_ref().unwrap().contains("PASS"));

    let meta = reopened.get_worktree_metadata(&task_id).unwrap().unwrap();
    assert_eq!(meta.0, format!("orc/task/{}", task_id));
    assert_eq!(meta.1, format!(".orc/worktrees/{}", task_id));
}

#[test]
fn test_existing_normal_run_submit_remains_green() {
    let (dir, db, task_id) = setup_test_env();
    agent::dispatch_manual(&task_id, &manual_agent(), &db, dir.path()).unwrap();
    let run_id = db.list_agent_runs_for_task(&task_id).unwrap()[0].id;

    let result = submit_run(&db, run_id, "architecture review completed: approved");
    assert!(result.is_ok());

    let task = db.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Review);

    let run = db.get_agent_run(run_id).unwrap().unwrap();
    assert_eq!(run.status, "completed");
    assert_eq!(
        run.output.as_deref(),
        Some("architecture review completed: approved")
    );
}
