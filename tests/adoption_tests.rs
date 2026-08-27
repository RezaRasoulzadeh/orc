use orc::adoption;
use orc::contract::DEFAULT_ENGINEERING_CONTRACT;
use orc::discovery;
use orc::lead::{LeadBackend, LeadBackendResponse, LeadContext, LeadDecision, LeadDecisionKind};
use orc::protocol::{
    DiscoveryArchitecture, DiscoveryEngineering, DiscoveryProject, DiscoveryState,
    PROTOCOL_VERSION, ProjectDiscoveryResponse,
};
use orc::storage::Database;
use std::cell::RefCell;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn git_repo() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("sample-project");
    std::fs::create_dir_all(&repo).unwrap();
    Command::new("git")
        .current_dir(&repo)
        .args(["init", "."])
        .output()
        .unwrap();
    (dir, repo)
}

fn response() -> ProjectDiscoveryResponse {
    ProjectDiscoveryResponse {
        protocol_version: PROTOCOL_VERSION,
        project: DiscoveryProject {
            name: "sample-project".into(),
            purpose: "A test repository".into(),
            languages: vec!["Rust".into()],
        },
        architecture: DiscoveryArchitecture {
            entry_points: vec!["src/main.rs".into()],
            modules: vec!["storage".into()],
            boundaries: vec!["CLI to library".into()],
        },
        engineering: DiscoveryEngineering {
            build_commands: vec!["cargo build".into()],
            test_commands: vec!["cargo test".into()],
            observed_patterns: vec!["small modules".into()],
        },
        state: DiscoveryState {
            implemented: vec!["CLI".into()],
            in_progress: vec!["adoption".into()],
            risks: vec!["unknown deployment environment".into()],
        },
        unknowns: vec!["production hosting".into()],
    }
}

#[test]
fn adopt_initializes_existing_git_repository() {
    let (_dir, repo) = git_repo();
    let root = adoption::adopt(&repo).unwrap();
    assert_eq!(root, std::fs::canonicalize(&repo).unwrap());
    assert!(repo.join(".orc/orc.db").exists());
    assert!(repo.join(".orc/engineering.md").exists());
    assert!(repo.join(".orc/project.md").exists());
    let db = Database::open(repo.join(".orc/orc.db")).unwrap();
    assert_eq!(
        db.get_project_name().unwrap().as_deref(),
        Some("sample-project")
    );
}

#[test]
fn adopt_writes_the_maintained_engineering_contract_when_missing() {
    let (_dir, repo) = git_repo();

    adoption::adopt(&repo).unwrap();

    assert_eq!(
        std::fs::read_to_string(repo.join(".orc/engineering.md")).unwrap(),
        DEFAULT_ENGINEERING_CONTRACT
    );
}

#[test]
fn adopt_fails_outside_git_repository() {
    let dir = TempDir::new().unwrap();
    let error = adoption::adopt(dir.path()).unwrap_err().to_string();
    assert!(error.contains("not inside a Git repository"));
}

#[test]
fn adopt_does_not_overwrite_existing_project_docs() {
    let (_dir, repo) = git_repo();
    std::fs::create_dir_all(repo.join(".orc")).unwrap();
    std::fs::write(repo.join(".orc/project.md"), "custom project notes\n").unwrap();
    adoption::adopt(&repo).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join(".orc/project.md")).unwrap(),
        "custom project notes\n"
    );
}

#[test]
fn adopt_does_not_overwrite_existing_engineering_contract() {
    let (_dir, repo) = git_repo();
    std::fs::create_dir_all(repo.join(".orc")).unwrap();
    std::fs::write(
        repo.join(".orc/engineering.md"),
        "# User-owned engineering contract\n",
    )
    .unwrap();
    adoption::adopt(&repo).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.join(".orc/engineering.md")).unwrap(),
        "# User-owned engineering contract\n"
    );
}

#[test]
fn adoption_preserves_empty_engineering_contract_but_populates_empty_project_doc() {
    let (_dir, repo) = git_repo();
    std::fs::create_dir_all(repo.join(".orc")).unwrap();
    std::fs::write(repo.join(".orc/engineering.md"), "").unwrap();
    std::fs::write(repo.join(".orc/project.md"), "").unwrap();

    adoption::adopt(&repo).unwrap();

    assert_eq!(
        std::fs::read_to_string(repo.join(".orc/engineering.md")).unwrap(),
        ""
    );
    assert!(
        std::fs::read_to_string(repo.join(".orc/project.md"))
            .unwrap()
            .contains("# Project")
    );
}

#[test]
fn discovery_request_is_json_and_read_only() {
    let (_dir, repo) = git_repo();
    adoption::adopt(&repo).unwrap();
    let request = discovery::build_request(&repo).unwrap();
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("Do not modify files"));
    assert!(json.contains("Do not run destructive commands"));
    assert!(json.contains("structured ProjectDiscoveryResponse"));
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
}

#[test]
fn discovery_response_deserializes_and_applies_summaries() {
    let (_dir, repo) = git_repo();
    adoption::adopt(&repo).unwrap();
    let original = response();
    let json = serde_json::to_string(&original).unwrap();
    let parsed = discovery::parse_response(&json).unwrap();
    discovery::apply_response(&repo, &parsed).unwrap();
    let project = std::fs::read_to_string(repo.join(".orc/project.md")).unwrap();
    let architecture = std::fs::read_to_string(repo.join(".orc/architecture.md")).unwrap();
    let roadmap = std::fs::read_to_string(repo.join(".orc/roadmap.md")).unwrap();
    assert!(project.contains("A test repository"));
    assert!(architecture.contains("src/main.rs"));
    assert!(roadmap.contains("production hosting"));
    assert!(roadmap.contains("does not create future tasks"));
    let db = Database::open(repo.join(".orc/orc.db")).unwrap();
    assert_eq!(
        db.get_project_name().unwrap().as_deref(),
        Some("sample-project")
    );
}

#[test]
fn discovery_snapshot_is_read_only_and_contains_project_context() {
    let (_dir, repo) = git_repo();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\ndescription = \"Example project\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
    adoption::adopt(&repo).unwrap();
    let before = std::fs::read(repo.join(".orc/orc.db")).unwrap();
    let snapshot = discovery::build_snapshot(&repo).unwrap();
    assert_eq!(snapshot.project.name, "sample-project");
    assert_eq!(
        snapshot.project.description.as_deref(),
        Some("Example project")
    );
    assert_eq!(snapshot.technology_stack, vec!["Rust"]);
    assert_eq!(snapshot.architecture.entry_points, vec!["src/main.rs"]);
    assert!(snapshot.important_files.contains(&"Cargo.toml".to_owned()));
    assert!(!snapshot.validation_commands.is_empty());
    assert_eq!(before, std::fs::read(repo.join(".orc/orc.db")).unwrap());
}

#[test]
fn reopening_database_preserves_adopted_project() {
    let (_dir, repo) = git_repo();
    adoption::adopt(&repo).unwrap();
    let db_path = repo.join(".orc/orc.db");
    drop(Database::open(&db_path).unwrap());
    let reopened = Database::open(db_path).unwrap();
    assert_eq!(
        reopened.get_project_name().unwrap().as_deref(),
        Some("sample-project")
    );
}

struct RecordingLead(RefCell<Option<LeadContext>>);

impl LeadBackend for RecordingLead {
    fn invoke(
        &self,
        context: &LeadContext,
        objective: &str,
    ) -> Result<LeadBackendResponse, String> {
        assert_eq!(objective, "understand this repository");
        self.0.borrow_mut().replace(context.clone());
        Ok(LeadBackendResponse {
            message: "assessment complete".into(),
            proposals: Vec::new(),
            decision: Some(LeadDecision {
                kind: LeadDecisionKind::PlanRequired,
                details: serde_json::json!({"reason": "new project"}),
            }),
        })
    }
}

struct CountingLead(RefCell<usize>);

impl LeadBackend for CountingLead {
    fn invoke(
        &self,
        _context: &LeadContext,
        _objective: &str,
    ) -> Result<LeadBackendResponse, String> {
        *self.0.borrow_mut() += 1;
        Ok(LeadBackendResponse {
            message: "unexpected invocation".into(),
            proposals: Vec::new(),
            decision: None,
        })
    }
}

#[test]
fn adopt_and_invoke_lead_discovers_persists_and_does_not_create_work() {
    let (_dir, repo) = git_repo();
    let backend = RecordingLead(RefCell::new(None));
    let (root, response) =
        adoption::adopt_and_invoke_lead(repo.join("."), "understand this repository", &backend, 20)
            .unwrap();
    assert!(backend.0.borrow().as_ref().unwrap().discovery.is_some());
    assert!(response.decision.is_some());
    let db = Database::open(root.join(".orc/orc.db")).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    assert_eq!(db.list_tasks().unwrap().len(), 0);
    assert_eq!(db.list_lead_decisions(project).unwrap().len(), 1);
    assert_eq!(
        Database::open(root.join(".orc/orc.db"))
            .unwrap()
            .list_lead_decisions(project)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn adopt_resolves_nested_repository_root_for_lead() {
    let (_dir, repo) = git_repo();
    let nested = repo.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let backend = RecordingLead(RefCell::new(None));
    let (root, _) =
        adoption::adopt_and_invoke_lead(&nested, "understand this repository", &backend, 20)
            .unwrap();
    assert_eq!(root, std::fs::canonicalize(repo).unwrap());
    assert_eq!(
        backend.0.borrow().as_ref().unwrap().repository_path,
        root.display().to_string()
    );
}

#[test]
fn adopt_aborts_before_lead_when_structured_discovery_fails() {
    let (_dir, repo) = git_repo();
    adoption::adopt(&repo).unwrap();
    std::fs::create_dir(repo.join(".orc/validation.toml")).unwrap();
    let backend = CountingLead(RefCell::new(0));

    let error = adoption::adopt_and_invoke_lead(&repo, "understand this repository", &backend, 20)
        .unwrap_err()
        .to_string();

    assert!(error.contains("failed to read"), "{error}");
    assert_eq!(*backend.0.borrow(), 0);
    let db = Database::open(repo.join(".orc/orc.db")).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    assert!(db.list_lead_decisions(project).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn adopt_cli_discovers_invokes_lead_from_nested_directory_and_only_persists_decision() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, repo) = git_repo();
    let nested = repo.join("src").join("nested");
    fs::create_dir_all(&nested).unwrap();
    let bin = repo.join(".fake-bin");
    fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex");
    fs::write(
        &codex,
        r##"#!/bin/sh
printf '%s\n' '{"message":"assessment","proposals":[],"decision":{"kind":"DIRECT_TASKS","details":{"tasks":[]}}}'
"##,
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), old_path.to_string_lossy());
    let output = Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(&nested)
        .env("PATH", path)
        .args(["adopt", "assess this existing repository"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let db = Database::open(repo.join(".orc/orc.db")).unwrap();
    let project = db.get_project_id().unwrap().unwrap();
    let decisions = db.list_lead_decisions(project).unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("assessment"));
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].source_request,
        "assess this existing repository"
    );
    assert!(decisions[0].snapshot.is_some());
    assert!(db.list_tasks().unwrap().is_empty());
    assert!(db.list_agent_runs(project, usize::MAX).unwrap().is_empty());
}
