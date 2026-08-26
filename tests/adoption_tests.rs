use orc::adoption;
use orc::contract::DEFAULT_ENGINEERING_CONTRACT;
use orc::discovery;
use orc::protocol::{
    DiscoveryArchitecture, DiscoveryEngineering, DiscoveryProject, DiscoveryState,
    PROTOCOL_VERSION, ProjectDiscoveryResponse,
};
use orc::storage::Database;
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
