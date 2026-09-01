use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use orc::protocol::{PlanResponse, TaskProposal};
use orc::storage::Database;
use tempfile::tempdir;

const EXPECTED_SEED_COMMIT: &str = "149d45a00b6d25d7bebcbcfaed398c59231dd376";
const VALIDATION: [&str; 3] = [
    "cargo fmt --check",
    "cargo clippy --all-targets -- -D warnings",
    "cargo test",
];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/autonomy/pocket-ledger-v1")
}

fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn generated_repo(parent: &Path, name: &str) -> PathBuf {
    let target = parent.join(name);
    run(Command::new(fixture().join("scripts/create-repository.sh")).arg(&target));
    target
}

#[test]
fn generator_is_deterministic_clean_and_contains_the_expected_tree() {
    let directory = tempdir().unwrap();
    let first = generated_repo(directory.path(), "first");
    let second = generated_repo(directory.path(), "second");

    for repository in [&first, &second] {
        let commit = run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "rev-parse",
            "HEAD",
        ]));
        assert_eq!(
            String::from_utf8_lossy(&commit.stdout).trim(),
            EXPECTED_SEED_COMMIT
        );
        let status = run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "status",
            "--porcelain",
            "--untracked-files=all",
        ]));
        assert!(status.stdout.is_empty());
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            ".orc/validation.toml",
            "src/model.rs",
            "src/parser.rs",
            "src/normalize.rs",
            "src/catalog.rs",
            "src/summary.rs",
            "src/bin/ledger_summary.rs",
            "tests/ledger_tests.rs",
        ] {
            assert!(repository.join(path).is_file(), "missing {path}");
        }
    }

    let existing = Command::new(fixture().join("scripts/create-repository.sh"))
        .arg(&first)
        .output()
        .unwrap();
    assert!(!existing.status.success());
    assert!(String::from_utf8_lossy(&existing.stderr).contains("target already exists"));
}

#[test]
fn five_contracts_parse_validate_and_preserve_ordered_semantics() {
    let mut tasks = Vec::new();
    for index in 1..=5 {
        let id = format!("T-{index:04}");
        let data = fs::read_to_string(fixture().join(format!("tasks/{id}.json"))).unwrap();
        let task: TaskProposal = serde_json::from_str(&data).unwrap();
        assert_eq!(task.local_id, id);
        assert_eq!(task.validation, VALIDATION);
        assert!(task.depends_on.is_empty());
        assert!(!task.acceptance_criteria.is_empty());
        tasks.push(task);
    }
    assert_eq!(tasks.len(), 5);

    let plan = PlanResponse {
        protocol_version: 1,
        objective: "Reproduce pocket-ledger-v1".into(),
        assumptions: vec!["Authentication is external".into()],
        risks: vec!["Semantic Review is under test".into()],
        questions: Vec::new(),
        tasks,
    };
    plan.validate().unwrap();

    let task_five = &plan.tasks[4];
    assert_eq!(task_five.local_id, "T-0005");
    assert!(task_five.objective.contains("Active, Inactive, Suspended"));
    assert!(task_five.acceptance_criteria.iter().any(|criterion| {
        criterion.contains("Suspended records must not be counted as active")
            && criterion.contains("active counts only RecordState::Active")
    }));
    let seed_tests =
        fs::read_to_string(fixture().join("repository/tests/ledger_tests.rs")).unwrap();
    assert!(!seed_tests.contains("Suspended"));

    let original = fs::read(fixture().join("tasks/T-0005.json")).unwrap();
    let reread = fs::read(fixture().join("tasks/T-0005.json")).unwrap();
    assert_eq!(
        original, reread,
        "contract bytes and criterion order must be stable"
    );
}

#[test]
fn validation_agent_economy_and_provenance_metadata_are_explicit_and_secret_free() {
    let fixture_metadata: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fixture().join("fixture.json")).unwrap()).unwrap();
    assert_eq!(fixture_metadata["fixture_version"], "pocket-ledger-v1");
    assert_eq!(
        fixture_metadata["expected_seed_baseline_commit"],
        EXPECTED_SEED_COMMIT
    );
    assert_eq!(fixture_metadata["task_count"], 5);
    assert_eq!(fixture_metadata["credentials_committed"], false);
    assert_eq!(
        fixture_metadata["byte_identical_to_lost_ephemeral_fixture"],
        false
    );

    let validation = fs::read_to_string(fixture().join("repository/.orc/validation.toml")).unwrap();
    for command in VALIDATION {
        assert!(validation.contains(command));
    }

    let config = fs::read_to_string(fixture().join("trial.env")).unwrap();
    for expected in [
        "pocket-ledger-v1",
        "trial-cheap",
        "gpt-5.6-luna",
        "trial-strong",
        "gpt-5.6-terra",
        "code review",
        "CODEX_HOME",
    ] {
        assert!(config.contains(expected));
    }
    let lower = config.to_ascii_lowercase();
    for forbidden in ["access_token", "refresh_token", "password", "api_key"] {
        assert!(!lower.contains(forbidden));
    }

    let provenance = fs::read_to_string(fixture().join("provenance.md")).unwrap();
    assert!(provenance.contains("not claimed to be byte-for-byte identical"));
    assert!(provenance.contains("Exact material recovered"));
    assert!(provenance.contains("Reconstructed material"));
}

#[cfg(unix)]
#[test]
fn prepared_fixture_adopts_loads_tasks_and_exposes_default_economy_without_ai_calls() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir().unwrap();
    let target = directory.path().join("generated");
    let results = directory.path().join("results");
    let profile = directory.path().join("profile");
    let bin = directory.path().join("bin");
    fs::create_dir_all(&profile).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let fake_codex = bin.join("codex");
    fs::write(
        &fake_codex,
        "#!/bin/sh\nif [ \"$1\" = login ] && [ \"$2\" = status ]; then printf 'authenticated\\n'; exit 0; fi\necho 'AI invocation forbidden in fixture test' >&2\nexit 91\n",
    )
    .unwrap();
    fs::set_permissions(&fake_codex, fs::Permissions::from_mode(0o755)).unwrap();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!("{}:{}", bin.display(), old_path.to_string_lossy());

    run(Command::new(fixture().join("scripts/prepare-trial.sh"))
        .args([
            target.to_str().unwrap(),
            results.to_str().unwrap(),
            "test-run",
            env!("CARGO_BIN_EXE_orc"),
        ])
        .env("TRIAL_PROFILE_PATH", &profile)
        .env("PATH", path));

    let result_dir = results.join("test-run");
    let registry = result_dir.join("registry/agents.db");
    assert!(registry.is_file());
    assert!(result_dir.join("manifest.json").is_file());
    assert!(result_dir.join("task-plan.json").is_file());

    let database = Database::open(target.join(".orc/orc.db")).unwrap();
    let tasks = database.list_tasks().unwrap();
    assert_eq!(tasks.len(), 5);
    assert_eq!(tasks[0].id, "T-0001");
    assert_eq!(tasks[4].id, "T-0005");
    assert!(tasks[4].objective.contains("Suspended"));

    let economy = run(Command::new(env!("CARGO_BIN_EXE_orc"))
        .current_dir(&target)
        .env("ORC_GLOBAL_REGISTRY_PATH", &registry)
        .args(["economy", "show"]));
    let economy: serde_json::Value = serde_json::from_slice(&economy.stdout).unwrap();
    assert_eq!(economy["configuration"]["model_costs"]["gpt-5.6-luna"], 1.0);
    assert_eq!(
        economy["configuration"]["model_costs"]["gpt-5.6-terra"],
        2.0
    );

    let agents = fs::read_to_string(result_dir.join("agents.txt")).unwrap();
    assert!(agents.contains("trial-cheap") && agents.contains("trial-strong"));
    let schedule = fs::read_to_string(result_dir.join("schedule-T-0001.txt")).unwrap();
    assert!(schedule.contains("Selected: trial-cheap"));
    assert!(schedule.contains("economy tier: default"));

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(result_dir.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["fixture_version"], "pocket-ledger-v1");
    assert_eq!(manifest["seed_baseline_commit"], EXPECTED_SEED_COMMIT);
    assert_eq!(manifest["trial_run_id"], "test-run");
    assert_eq!(
        manifest["task_contract_sha256"].as_object().unwrap().len(),
        5
    );

    let engineering = fs::read_to_string(target.join(".orc/engineering.md")).unwrap();
    assert!(engineering.contains("default contract for work performed in an adopted repository"));
    assert!(!engineering.contains("src/App.vue"));
    let status = run(Command::new("git").args([
        "-C",
        target.to_str().unwrap(),
        "status",
        "--porcelain",
        "--untracked-files=all",
    ]));
    assert!(status.stdout.is_empty());
}
