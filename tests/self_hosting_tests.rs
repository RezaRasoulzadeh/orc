use std::path::Path;
use std::process::Command;

use orc::automated::{ActionOverrides, resolve_action, validation_evidence_fingerprint};
use orc::operations::ProjectOperations;
use orc::registry::{AUTOMATED, AVAILABLE, AgentAction, AgentDefinition, ReasoningEffort};
use orc::self_hosting::{
    ORC_REPOSITORY_ID, PROJECT_IDENTITY_PATH, SELF_HOSTED_MUTATION_ENV, SelfHostingReadinessState,
};
use orc::storage::Database;

fn git(repository: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repository)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository(parent: &Path, name: &str, with_orc_identity: bool) -> std::path::PathBuf {
    let root = parent.join(name);
    std::fs::create_dir_all(root.join(".orc")).unwrap();
    git(&root, &["init", "."]);
    git(&root, &["config", "user.email", "self-hosting@example.com"]);
    git(&root, &["config", "user.name", "Self Hosting Tests"]);
    std::fs::write(root.join("README.md"), "repository\n").unwrap();
    std::fs::write(root.join(".gitignore"), ".orc/orc.db*\n.orc/worktrees/\n").unwrap();
    if with_orc_identity {
        std::fs::write(
            root.join(PROJECT_IDENTITY_PATH),
            format!(
                "{{\n  \"schema_version\": 1,\n  \"repository_id\": \"{ORC_REPOSITORY_ID}\"\n}}\n"
            ),
        )
        .unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "base"]);
    root
}

fn coder() -> AgentDefinition {
    AgentDefinition {
        id: "economy-coder".into(),
        backend: "codex".into(),
        execution_mode: AUTOMATED.into(),
        display_name: "Economy Coder".into(),
        enabled: true,
        priority: 1,
        capabilities: vec!["code".into(), "command_execution".into()],
        status: AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: Some("/tmp/self-hosting-test-profile".into()),
        model: Some("luna".into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        config_metadata: None,
        quota_remaining_percent: Some(80),
        quota_reset_at: None,
        quota_checked_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        ),
        quota_source: Some("test".into()),
        quota_limits: None,
        actions: vec![AgentAction::Code],
    }
}

#[test]
fn identity_is_path_independent_and_external_projects_are_not_self_hosting() {
    let directory = tempfile::tempdir().unwrap();
    let first = repository(directory.path(), "first-checkout", true);
    let nested_parent = directory.path().join("different/absolute/location");
    std::fs::create_dir_all(&nested_parent).unwrap();
    let second = repository(&nested_parent, "renamed-checkout", true);
    let external = repository(directory.path(), "external-orc-name", false);

    let first = orc::self_hosting::inspect(first);
    let second = orc::self_hosting::inspect(second);
    assert!(first.recognized && second.recognized);
    assert_eq!(first.repository_id, second.repository_id);
    assert_eq!(first.state, SelfHostingReadinessState::Ready);
    assert_eq!(second.state, SelfHostingReadinessState::Ready);

    let external = orc::self_hosting::inspect(external);
    assert!(!external.recognized);
    assert_eq!(external.state, SelfHostingReadinessState::NotApplicable);
}

#[test]
fn project_reopen_preserves_self_hosting_observability() {
    let directory = tempfile::tempdir().unwrap();
    let root = repository(directory.path(), "orc", true);
    let database_path = root.join(".orc/orc.db");
    let db = Database::init(&database_path).unwrap();
    db.create_project("Orc").unwrap();
    let before = ProjectOperations::new(&db, &root)
        .snapshot()
        .unwrap()
        .self_hosting;
    drop(db);

    let reopened = Database::open(&database_path).unwrap();
    let after = ProjectOperations::new(&reopened, &root)
        .snapshot()
        .unwrap()
        .self_hosting;
    assert_eq!(after, before);
    assert!(after.recognized);
    assert_eq!(after.state, SelfHostingReadinessState::Ready);
}

#[test]
fn self_hosting_identity_does_not_change_economy_resolution() {
    let directory = tempfile::tempdir().unwrap();
    let root = repository(directory.path(), "economy", false);
    let db = Database::init(root.join(".orc/orc.db")).unwrap();
    db.create_project("economy").unwrap();
    db.insert_agent(&coder()).unwrap();
    let before = resolve_action(&db, AgentAction::Code, &ActionOverrides::default())
        .unwrap()
        .1;

    std::fs::write(
        root.join(PROJECT_IDENTITY_PATH),
        format!("{{\"schema_version\":1,\"repository_id\":\"{ORC_REPOSITORY_ID}\"}}\n"),
    )
    .unwrap();
    git(&root, &["add", PROJECT_IDENTITY_PATH]);
    git(&root, &["commit", "-m", "identify repository"]);
    assert!(orc::self_hosting::inspect(&root).recognized);
    let after = resolve_action(&db, AgentAction::Code, &ActionOverrides::default())
        .unwrap()
        .1;

    assert_eq!(after.agent, before.agent);
    assert_eq!(after.model, before.model);
    assert_eq!(after.reasoning_effort, before.reasoning_effort);
    assert_eq!(after.resolution_record.tier, before.resolution_record.tier);
}

#[test]
fn task_worktree_isolation_runtime_evidence_and_process_guard_are_deterministic() {
    let directory = tempfile::tempdir().unwrap();
    let root = repository(directory.path(), "orc", true);
    let (_, first_path) = orc::git::ensure_worktree("T-0001", &root).unwrap();
    let (_, second_path) = orc::git::ensure_worktree("T-0002", &root).unwrap();
    let first = orc::git::validate_task_worktree(&root, "T-0001", &first_path).unwrap();
    assert!(orc::git::validate_task_worktree(&root, "T-0001", &second_path).is_err());
    assert!(orc::git::validate_task_worktree(&root, "T-0001", &root).is_err());

    std::fs::write(first.join("implementation.rs"), "pub fn implemented() {}\n").unwrap();
    std::fs::write(first.join(".orc/orc.db"), "runtime database").unwrap();
    std::fs::write(first.join(".orc/orc.agents.db-wal"), "global registry").unwrap();
    std::fs::create_dir_all(first.join(".orc/history")).unwrap();
    std::fs::write(first.join(".orc/history/session.log"), "runtime log").unwrap();
    std::fs::write(
        first.join(".orc/validation.toml"),
        "commands = [\"true\"]\n",
    )
    .unwrap();
    let changes = orc::git::inspect_worktree(&first, &root).unwrap();
    let paths = changes
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"implementation.rs"));
    assert!(paths.contains(&".orc/validation.toml"));
    assert!(!paths.iter().any(|path| path.contains("orc.db")));
    assert!(!paths.iter().any(|path| path.starts_with(".orc/history")));

    // A task cannot turn off the inherited recursion guard by editing the
    // working copy of the committed identity marker.
    std::fs::write(
        first.join(PROJECT_IDENTITY_PATH),
        "{\"schema_version\":1,\"repository_id\":\"different\"}\n",
    )
    .unwrap();
    let mut command = Command::new("provider");
    orc::self_hosting::mark_mutation_process(&mut command, &first);
    assert!(command.get_envs().any(|(key, value)| {
        key == SELF_HOSTED_MUTATION_ENV && value.is_some_and(|value| value == "1")
    }));
}

#[test]
fn validation_fingerprint_is_bound_to_task_worktree_and_diff() {
    let changes = orc::git::WorktreeChanges {
        files: vec![orc::git::ChangedFile {
            status: "M".into(),
            path: "src/lib.rs".into(),
        }],
        stat: "1 file changed".into(),
        diff: "diff --git a/src/lib.rs b/src/lib.rs\n".into(),
    };
    let current = validation_evidence_fingerprint("T-0001", ".orc/worktrees/T-0001", &changes);
    assert_ne!(
        current,
        validation_evidence_fingerprint("T-0002", ".orc/worktrees/T-0002", &changes)
    );
    let mut changed = changes;
    changed.diff.push_str("+changed\n");
    assert_ne!(
        current,
        validation_evidence_fingerprint("T-0001", ".orc/worktrees/T-0001", &changed)
    );
}

#[test]
fn self_hosted_mutation_process_cannot_recursively_invoke_orc() {
    let output = Command::new(env!("CARGO_BIN_EXE_orc"))
        .arg("status")
        .env(SELF_HOSTED_MUTATION_ENV, "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("recursive Orc invocation is disabled")
    );
}
