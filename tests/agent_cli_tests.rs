use std::process::Command;

use orc::storage::Database;
use tempfile::tempdir;

fn orc_command(directory: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn codex_add_profile_and_profile_update_persist() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    assert!(
        orc_command(
            directory.path(),
            &[
                "agent",
                "add",
                "codex-third",
                "--backend",
                "codex",
                "--profile",
                "/profiles/third",
            ],
        )
        .status
        .success()
    );
    assert!(
        orc_command(
            directory.path(),
            &["agent", "profile", "codex-third", "/profiles/updated-third"],
        )
        .status
        .success()
    );

    let db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    assert_eq!(
        db.get_agent("codex-third")
            .unwrap()
            .unwrap()
            .profile_path
            .as_deref(),
        Some("/profiles/updated-third")
    );
}

#[test]
fn agent_profile_rejects_missing_agent() {
    let directory = tempdir().unwrap();
    assert!(orc_command(directory.path(), &["init"]).status.success());
    let output = orc_command(
        directory.path(),
        &["agent", "profile", "unknown", "/profiles/unknown"],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown"));
}
