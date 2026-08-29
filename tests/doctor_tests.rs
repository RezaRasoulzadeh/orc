use std::cell::RefCell;
use std::path::Path;
use std::process::Command;

use orc::backend::HealthCommandRunner;
use orc::doctor::{self, CheckStatus};
use orc::registry::{AVAILABLE, AgentDefinition};
use orc::storage::Database;
use tempfile::TempDir;

struct FakeRunner {
    commands: RefCell<Vec<(String, Vec<String>)>>,
    codex_exists: bool,
    agy_exists: bool,
}

impl HealthCommandRunner for FakeRunner {
    fn executable_exists(&self, executable: &str) -> bool {
        executable == "git"
            || (executable == "codex" && self.codex_exists)
            || (executable == "agy" && self.agy_exists)
    }

    fn run(
        &self,
        executable: &str,
        args: &[&str],
        _cwd: &Path,
        env: Option<(&str, &Path)>,
    ) -> Result<(), String> {
        let mut recorded = args.iter().map(|arg| (*arg).into()).collect::<Vec<_>>();
        if let Some((key, value)) = env {
            recorded.push(format!("{key}={}", value.display()));
        }
        self.commands
            .borrow_mut()
            .push((executable.into(), recorded));
        Ok(())
    }
}

fn project() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir_all(root.join(".orc")).unwrap();
    Command::new("git")
        .args(["init", root.to_str().unwrap()])
        .output()
        .unwrap();
    Database::init(root.join(".orc/orc.db")).unwrap();
    std::fs::write(root.join(".orc/engineering.md"), "contract").unwrap();
    (dir, root)
}

fn agent(backend: &str, profile: Option<String>) -> AgentDefinition {
    AgentDefinition {
        id: "agent".into(),
        backend: backend.into(),
        execution_mode: "automated".into(),
        display_name: "Agent".into(),
        enabled: true,
        priority: 1,
        capabilities: vec![],
        status: AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: profile,
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

#[test]
fn healthy_project_checks_auth_without_an_ai_request() {
    let (_dir, root) = project();
    let profile = root.join("profile");
    std::fs::create_dir(&profile).unwrap();
    let db = Database::open(root.join(".orc/orc.db")).unwrap();
    db.insert_agent(&agent("codex", Some(profile.display().to_string())))
        .unwrap();
    let runner = FakeRunner {
        commands: RefCell::new(vec![]),
        codex_exists: true,
        agy_exists: true,
    };
    let report = doctor::inspect(&root, &runner);
    assert_eq!(report.overall(), "OK");
    let commands = runner.commands.borrow();
    assert!(commands.iter().any(|(program, args)| program == "codex"
        && args
            == &[
                "login",
                "status",
                &format!("CODEX_HOME={}", profile.display())
            ]));
    assert!(
        !commands
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "exec" || arg == "-p"))
    );
}

#[test]
fn reports_missing_contract_database_unsupported_backend_and_profile() {
    let (_dir, root) = project();
    std::fs::remove_file(root.join(".orc/engineering.md")).unwrap();
    let db = Database::open(root.join(".orc/orc.db")).unwrap();
    db.insert_agent(&agent("unsupported", None)).unwrap();
    let runner = FakeRunner {
        commands: RefCell::new(vec![]),
        codex_exists: true,
        agy_exists: true,
    };
    let report = doctor::inspect(&root, &runner);
    assert!(
        report
            .project
            .iter()
            .any(|check| check.name == "engineering contract"
                && matches!(check.status, CheckStatus::Failed(_)))
    );
    assert!(matches!(
        report.agents[0].status,
        CheckStatus::Unavailable(_)
    ));
    drop(db);
    std::fs::remove_file(root.join(".orc/orc.db")).unwrap();
    let report = doctor::inspect(&root, &runner);
    assert!(report.project.iter().any(|check| check.name == "database" && matches!(check.status, CheckStatus::Failed(_))));

    let reopened = Database::init(root.join(".orc/orc.db")).unwrap();
    assert!(reopened.get_agent("agent").unwrap().is_some());
    reopened
        .set_agent_profile_path("agent", &root.join("missing").display().to_string())
        .unwrap();
    let report = doctor::inspect(&root, &runner);
    assert!(matches!(
        report.agents[0].status,
        CheckStatus::Unavailable(_)
    ));
}

#[test]
fn missing_provider_cli_is_reported_through_lookup_abstraction() {
    let (_dir, root) = project();
    Database::open(root.join(".orc/orc.db"))
        .unwrap()
        .insert_agent(&agent("codex", None))
        .unwrap();
    let runner = FakeRunner {
        commands: RefCell::new(vec![]),
        codex_exists: false,
        agy_exists: false,
    };
    let report = doctor::inspect(&root, &runner);
    assert!(matches!(
        report.agents[0].status,
        CheckStatus::Unavailable(_)
    ));
}

#[test]
fn missing_codex_profile_is_reported_without_running_login_status() {
    let (_dir, root) = project();
    Database::open(root.join(".orc/orc.db"))
        .unwrap()
        .insert_agent(&agent("codex", None))
        .unwrap();
    let runner = FakeRunner {
        commands: RefCell::new(vec![]),
        codex_exists: true,
        agy_exists: true,
    };
    let report = doctor::inspect(&root, &runner);
    assert!(
        matches!(report.agents[0].status, CheckStatus::Unavailable(ref error) if error.contains("agent") && error.contains("profile path"))
    );
    assert!(
        !runner
            .commands
            .borrow()
            .iter()
            .any(|(program, _)| program == "codex")
    );
}

#[test]
fn antigravity_health_check_uses_version_flag_without_an_ai_request() {
    let (_dir, root) = project();
    let db = Database::open(root.join(".orc/orc.db")).unwrap();
    db.insert_agent(&agent("antigravity", None)).unwrap();
    let runner = FakeRunner {
        commands: RefCell::new(vec![]),
        codex_exists: false,
        agy_exists: true,
    };
    let report = doctor::inspect(&root, &runner);
    assert_eq!(report.overall(), "OK");
    let commands = runner.commands.borrow();
    assert!(
        commands
            .iter()
            .any(|(program, args)| program == "agy" && args == &["--version"])
    );
    assert!(
        !commands
            .iter()
            .any(|(_, args)| args.iter().any(|arg| arg == "-p"))
    );
}

#[test]
fn missing_agy_executable_is_reported_as_unavailable() {
    let (_dir, root) = project();
    Database::open(root.join(".orc/orc.db"))
        .unwrap()
        .insert_agent(&agent("antigravity", None))
        .unwrap();
    let runner = FakeRunner {
        commands: RefCell::new(vec![]),
        codex_exists: true,
        agy_exists: false,
    };
    let report = doctor::inspect(&root, &runner);
    assert!(matches!(
        report.agents[0].status,
        CheckStatus::Unavailable(_)
    ));
}
