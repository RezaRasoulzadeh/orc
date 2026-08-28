use orc::storage::Database;
use orc::task::TaskStatus;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run_orc(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(root)
        .args(args)
        .output()
        .expect("run orc")
}

fn assert_orc(root: &Path, args: &[&str]) -> String {
    let output = run_orc(root, args);
    assert!(
        output.status.success(),
        "orc {:?} failed:\n{}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("orc output is utf-8")
}

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("run git")
}

fn init_git_repository(root: &Path) {
    assert!(git(root, &["init", "."]).status.success());
    assert!(
        git(root, &["config", "user.email", "test@example.com"])
            .status
            .success()
    );
    assert!(
        git(root, &["config", "user.name", "Orc Acceptance"])
            .status
            .success()
    );
    fs::write(root.join("README.md"), "clean repository\n").expect("write readme");
    fs::write(root.join(".gitignore"), ".orc/\n").expect("write gitignore");
    assert!(git(root, &["add", "."]).status.success());
    assert!(git(root, &["commit", "-m", "initial"]).status.success());
}

#[test]
fn v01_happy_path_is_provider_independent() {
    let directory = TempDir::new().expect("temporary repository");
    init_git_repository(directory.path());

    assert_orc(directory.path(), &["init"]);
    assert_orc(directory.path(), &["adopt"]);
    fs::write(
        directory.path().join(".orc/validation.toml"),
        "commands = [\"test -f accepted.txt\"]\n",
    )
    .expect("write validation configuration");

    let plan = serde_json::json!({
        "protocol_version": 1,
        "objective": "Accept a controlled manual change",
        "assumptions": [],
        "risks": [],
        "questions": [],
        "tasks": [{
            "local_id": "T-0001",
            "title": "Add accepted marker",
            "objective": "Create accepted.txt through the manual task workflow",
            "role": "developer",
            "priority": "normal",
            "capabilities": [],
            "scope_mode": null,
            "context_files": [],
            "expected_changes": ["accepted.txt"],
            "unchanged": ["unrelated behavior"],
            "acceptance_criteria": ["accepted marker is created"],
            "required_tests": ["manual workflow test"],
            "validation": ["cargo test"],
            "execution_hints": {"effort":"low","effort_reason":"isolated marker creation"},
            "risk_factors": [],
            "depends_on": []
        }]
    });
    fs::write(
        directory.path().join(".orc/plan.json"),
        serde_json::to_vec(&plan).expect("serialize plan"),
    )
    .expect("write plan");
    assert_orc(directory.path(), &["apply-plan", ".orc/plan.json"]);

    assert_orc(
        directory.path(),
        &[
            "agent",
            "add",
            "manual-coder",
            "--backend",
            "claude",
            "--mode",
            "manual",
            "--capability",
            "code",
            "--capability",
            "terminal",
        ],
    );
    let queue = assert_orc(directory.path(), &["queue", "--explain"]);
    assert!(queue.contains("T-0001"));
    assert!(queue.contains("READY"), "queue output: {queue}");

    let dispatch = assert_orc(
        directory.path(),
        &["dispatch", "T-0001", "--agent", "manual-coder"],
    );
    assert!(dispatch.contains("# Orc Manual Task Packet"));
    assert!(dispatch.contains("Agent ID: manual-coder"));
    assert!(dispatch.contains("Task ID: T-0001"));
    assert!(dispatch.contains("Required response / handoff format"));
    let run_id = dispatch
        .lines()
        .find_map(|line| line.strip_prefix("Run "))
        .and_then(|line| line.split_whitespace().next())
        .expect("manual dispatch reports run id")
        .to_owned();
    let db = Database::open(directory.path().join(".orc/orc.db")).expect("open database");
    let run = db
        .list_agent_runs_for_task("T-0001")
        .expect("list runs")
        .into_iter()
        .next()
        .expect("manual run");
    assert_eq!(run.id.to_string(), run_id);
    assert_eq!(run.status, "waiting_external");
    assert_eq!(
        db.get_task("T-0001")
            .expect("get task")
            .expect("task")
            .status,
        TaskStatus::Active
    );
    drop(db);

    let patch = "diff --git a/accepted.txt b/accepted.txt\nnew file mode 100644\n--- /dev/null\n+++ b/accepted.txt\n@@ -0,0 +1 @@\n+accepted through Orc\n";
    fs::write(directory.path().join(".orc/change.patch"), patch).expect("write patch");
    assert_orc(
        directory.path(),
        &["run", "submit-patch", &run_id, ".orc/change.patch"],
    );
    let db = Database::open(directory.path().join(".orc/orc.db")).expect("open database");
    assert_eq!(
        db.get_task("T-0001")
            .expect("get task")
            .expect("task")
            .status,
        TaskStatus::Review
    );
    drop(db);

    assert_orc(directory.path(), &["task", "accept", "T-0001"]);
    let db = Database::open(directory.path().join(".orc/orc.db")).expect("open database");
    assert_eq!(
        db.get_task("T-0001")
            .expect("get task")
            .expect("task")
            .status,
        TaskStatus::Done
    );
    drop(db);
    assert!(directory.path().join("accepted.txt").is_file());
    assert!(
        git(directory.path(), &["log", "--oneline", "--all"])
            .stdout
            .windows(b"Add accepted marker".len())
            .any(|window| window == b"Add accepted marker")
    );

    let doctor = assert_orc(directory.path(), &["doctor"]);
    assert!(doctor.contains("Active tasks\n  (none)"));
    assert!(doctor.contains("Overall: OK"), "doctor output: {doctor}");
}
