use std::process::Command;

use tempfile::tempdir;

#[test]
fn economy_cli_configures_tiers_and_emits_read_model_without_direct_database_access() {
    let directory = tempdir().unwrap();
    let registry = directory.path().join("global/agents.db");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_orc"))
            .current_dir(directory.path())
            .env("ORC_GLOBAL_REGISTRY_PATH", &registry)
            .args(args)
            .output()
            .unwrap()
    };

    let init = run(&["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let configured = run(&[
        "economy",
        "configure",
        "--model-cost",
        "model-cheap=1",
        "--model-cost",
        "model-strong=2",
        "--unknown-tier",
        "unknown",
    ]);
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );

    let shown = run(&["economy", "show"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(value["configuration"]["model_costs"]["model-cheap"], 1.0);
    assert_eq!(value["configuration"]["model_costs"]["model-strong"], 2.0);
    assert_eq!(value["configuration"]["unknown_tier"], "unknown");
    assert_eq!(value["economy"]["invocation_count"], 0);

    let context = run(&["economy", "context"]);
    assert!(
        context.status.success(),
        "{}",
        String::from_utf8_lossy(&context.stderr)
    );
    let context_value: serde_json::Value = serde_json::from_slice(&context.stdout).unwrap();
    assert_eq!(context_value, serde_json::json!([]));
    assert!(!String::from_utf8_lossy(&context.stdout).contains("Authoritative Orc packet"));
}
