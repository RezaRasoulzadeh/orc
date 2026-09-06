use orc::app::OrcApp;
use orc::controller_experience::{
    CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION, CONTROLLER_EXPERIENCE_SNAPSHOT_SCHEMA_VERSION,
    ControllerExperienceCorrectionMetadata, ControllerExperienceExample,
    ControllerExperienceExampleDraft, ControllerExperienceOutcome, ControllerExperienceProvenance,
    ControllerExperienceQuality, ControllerExperienceSnapshot,
    ControllerExperienceVerificationBasis,
};
use orc::storage::Database;
use rusqlite::{Connection, params};
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp, i64, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("project.db");
    let registry_path = directory.path().join(".orc/global.db");
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let database = Database::init_with_registry(&database_path, &registry_path).unwrap();
    let project_id = database.create_project("experience snapshot").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&database_path, directory.path(), &registry_path).unwrap();
    (directory, app, project_id, registry_path)
}

fn draft(project_id: i64, capability: &str) -> ControllerExperienceExampleDraft {
    ControllerExperienceExampleDraft {
        schema_version: CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
        capability: capability.into(),
        input: serde_json::json!({
            "project_id": project_id,
            "request": "preserve exactly"
        }),
        accepted_output: serde_json::json!({
            "decision": "accepted",
            "capability": capability
        }),
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance {
            project_id: Some(project_id),
            task_id: Some("T-SNAPSHOT".into()),
            run_id: Some(7),
            plan_id: Some(11),
            review_id: Some(13),
            memory_id: None,
            source_reference: Some("snapshot-test".into()),
        },
        correction: None,
        outcome: ControllerExperienceOutcome::Accepted,
        quality: ControllerExperienceQuality {
            score: 91,
            rationale: "explicit snapshot fixture".into(),
        },
    }
}

fn insert(app: &OrcApp, project_id: i64, capability: &str) -> ControllerExperienceExample {
    app.create_controller_experience_example(&draft(project_id, capability))
        .unwrap()
}

#[test]
fn empty_dataset_returns_valid_empty_snapshot() {
    let (_directory, app, _project_id, _registry) = fixture();
    let snapshot = app.controller_experience_snapshot().unwrap();
    assert_eq!(
        snapshot,
        ControllerExperienceSnapshot {
            schema_version: CONTROLLER_EXPERIENCE_SNAPSHOT_SCHEMA_VERSION,
            count: 0,
            examples: Vec::new(),
        }
    );
    snapshot.validate().unwrap();
}

#[test]
fn one_active_example_preserves_the_complete_canonical_record() {
    let (_directory, app, project_id, _registry) = fixture();
    let expected = insert(&app, project_id, "controller.snapshot");
    let snapshot = app.controller_experience_snapshot().unwrap();
    assert_eq!(snapshot.count, 1);
    assert_eq!(snapshot.examples, vec![expected]);
    snapshot.validate().unwrap();
}

#[test]
fn retired_only_dataset_returns_empty_snapshot() {
    let (_directory, app, project_id, _registry) = fixture();
    let retired = insert(&app, project_id, "controller.retired");
    app.retire_controller_experience_example(retired.id)
        .unwrap();
    let snapshot = app.controller_experience_snapshot().unwrap();
    assert_eq!(snapshot.count, 0);
    assert!(snapshot.examples.is_empty());
    snapshot.validate().unwrap();
}

#[test]
fn mixed_lifecycle_snapshot_contains_only_active_examples() {
    let (_directory, app, project_id, _registry) = fixture();
    let first = insert(&app, project_id, "controller.first");
    let retired = insert(&app, project_id, "controller.retired");
    let third = insert(&app, project_id, "controller.third");
    app.retire_controller_experience_example(retired.id)
        .unwrap();

    let snapshot = app.controller_experience_snapshot().unwrap();
    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.examples, vec![first, third]);
}

#[test]
fn active_examples_are_returned_in_strict_ascending_identity_order() {
    let (directory, app, project_id, registry) = fixture();
    let first = insert(&app, project_id, "controller.first");
    let second = insert(&app, project_id, "controller.second");
    let third = insert(&app, project_id, "controller.third");
    drop(app);

    let connection = Connection::open(&registry).unwrap();
    connection
        .execute(
            "UPDATE controller_experience_examples SET id = id + 1000",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE controller_experience_examples SET id = ?1 WHERE id = ?2",
            params![30_i64, first.id + 1000],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE controller_experience_examples SET id = ?1 WHERE id = ?2",
            params![10_i64, second.id + 1000],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE controller_experience_examples SET id = ?1 WHERE id = ?2",
            params![20_i64, third.id + 1000],
        )
        .unwrap();
    drop(connection);

    let app = OrcApp::open_with_registry(
        directory.path().join("project.db"),
        directory.path(),
        &registry,
    )
    .unwrap();
    let snapshot = app.controller_experience_snapshot().unwrap();
    assert_eq!(
        snapshot
            .examples
            .iter()
            .map(|example| example.id)
            .collect::<Vec<_>>(),
        vec![10, 20, 30]
    );
    snapshot.validate().unwrap();
}

#[test]
fn correction_and_provenance_are_preserved_exactly() {
    let (_directory, app, project_id, _registry) = fixture();
    let mut corrected = draft(project_id, "controller.corrected");
    corrected.accepted_output = serde_json::json!({
        "decision": "accepted after correction",
        "complete": {"field": [1, 2, 3]}
    });
    corrected.verification_basis = ControllerExperienceVerificationBasis::ExplicitCorrection;
    corrected.outcome = ControllerExperienceOutcome::Corrected;
    corrected.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({
            "decision": "observed",
            "complete": {"field": [1, 2]}
        }),
        operator_reference: "operator:snapshot".into(),
        reason: "explicit correction fixture".into(),
    });
    let expected = app
        .create_controller_experience_example(&corrected)
        .unwrap();

    let snapshot = app.controller_experience_snapshot().unwrap();
    assert_eq!(snapshot.examples, vec![expected]);
}

#[test]
fn all_fixed_m08_capabilities_are_preserved_without_aliasing() {
    let (_directory, app, project_id, _registry) = fixture();
    let capabilities = [
        orc::controller_experience_recommendation::CONTROLLER_RECOMMENDATION_EXPERIENCE_CAPABILITY,
        orc::controller_experience_recovery::CONTROLLER_RECOVERY_RECOMMENDATION_EXPERIENCE_CAPABILITY,
        orc::controller_experience_planning::CONTROLLER_PLAN_GENERATION_EXPERIENCE_CAPABILITY,
        orc::controller_experience_intake::CONTROLLER_WORKFLOW_INTAKE_EXPERIENCE_CAPABILITY,
        orc::controller_experience_plan_review::CONTROLLER_PLAN_REVIEW_EXPERIENCE_CAPABILITY,
        orc::controller_experience_plan_revision::CONTROLLER_PLAN_REVISION_EXPERIENCE_CAPABILITY,
        orc::controller_experience_memory_capture::CONTROLLER_MEMORY_CAPTURE_EXPERIENCE_CAPABILITY,
        orc::controller_experience_memory_maintenance::CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY,
        orc::controller_experience_memory_selection::CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY,
    ];
    for capability in capabilities {
        insert(&app, project_id, capability);
    }

    let snapshot = app.controller_experience_snapshot().unwrap();
    let mut actual = snapshot
        .examples
        .iter()
        .map(|example| example.capability.as_str())
        .collect::<Vec<_>>();
    actual.sort_unstable();
    let mut expected = capabilities.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    assert_eq!(snapshot.count, capabilities.len() as u64);
}

#[test]
fn snapshot_validation_rejects_duplicate_nonpositive_and_misordered_ids() {
    let (_directory, app, project_id, _registry) = fixture();
    let first = insert(&app, project_id, "controller.first");
    let second = insert(&app, project_id, "controller.second");

    let mut duplicate = ControllerExperienceSnapshot {
        schema_version: CONTROLLER_EXPERIENCE_SNAPSHOT_SCHEMA_VERSION,
        count: 2,
        examples: vec![first.clone(), first.clone()],
    };
    assert!(duplicate.validate().is_err());

    let wrong_count = ControllerExperienceSnapshot {
        schema_version: CONTROLLER_EXPERIENCE_SNAPSHOT_SCHEMA_VERSION,
        count: 1,
        examples: vec![first.clone(), second.clone()],
    };
    assert!(wrong_count.validate().is_err());

    duplicate.examples = vec![ControllerExperienceExample {
        id: 0,
        ..first.clone()
    }];
    duplicate.count = 1;
    assert!(duplicate.validate().is_err());

    let misordered = ControllerExperienceSnapshot {
        schema_version: CONTROLLER_EXPERIENCE_SNAPSHOT_SCHEMA_VERSION,
        count: 2,
        examples: vec![second, first],
    };
    assert!(misordered.validate().is_err());
}

fn assert_corrupt_row_fails(update: impl FnOnce(&Connection)) {
    let (directory, app, project_id, registry) = fixture();
    insert(&app, project_id, "controller.corrupt");
    drop(app);
    let connection = Connection::open(&registry).unwrap();
    update(&connection);
    drop(connection);
    let reopened = OrcApp::open_with_registry(
        directory.path().join("project.db"),
        directory.path(),
        &registry,
    )
    .unwrap();
    assert!(reopened.controller_experience_snapshot().is_err());
}

#[test]
fn malformed_persisted_rows_fail_closed() {
    assert_corrupt_row_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET schema_version = ?1",
                params![999_i64],
            )
            .unwrap();
    });
    assert_corrupt_row_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET verification_basis = ?1",
                params!["unknown_basis"],
            )
            .unwrap();
    });
    assert_corrupt_row_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET outcome = ?1",
                params!["unknown_outcome"],
            )
            .unwrap();
    });
    assert_corrupt_row_fails(|connection| {
        connection
            .execute_batch("PRAGMA ignore_check_constraints = ON")
            .unwrap();
        connection
            .execute(
                "UPDATE controller_experience_examples SET lifecycle = ?1",
                params!["archived"],
            )
            .unwrap();
    });
    assert_corrupt_row_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET capability = ?1",
                params![""],
            )
            .unwrap();
    });
    assert_corrupt_row_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET input_payload = ?1",
                params!["{"],
            )
            .unwrap();
    });
}

#[test]
fn repeated_serialized_snapshots_are_identical_and_reads_write_nothing() {
    let (_directory, app, project_id, registry) = fixture();
    insert(&app, project_id, "controller.repeat");
    let before = std::fs::read(&registry).unwrap();
    let first = app.controller_experience_snapshot().unwrap();
    let second = app.controller_experience_snapshot().unwrap();
    let after = std::fs::read(&registry).unwrap();

    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    assert_eq!(before, after);
}

#[test]
fn retiring_one_example_removes_only_that_example_from_the_next_snapshot() {
    let (_directory, app, project_id, _registry) = fixture();
    let retained = insert(&app, project_id, "controller.retained");
    let removed = insert(&app, project_id, "controller.removed");
    let before = app.controller_experience_snapshot().unwrap();
    app.retire_controller_experience_example(removed.id)
        .unwrap();
    let after = app.controller_experience_snapshot().unwrap();

    assert_eq!(before.count, 2);
    assert_eq!(after.count, 1);
    assert_eq!(after.examples, vec![retained]);
}
