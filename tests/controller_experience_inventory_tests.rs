use orc::app::OrcApp;
use orc::controller_experience::{
    CONTROLLER_EXPERIENCE_INVENTORY_SCHEMA_VERSION, ControllerExperienceCapabilitySummary,
    ControllerExperienceExampleDraft, ControllerExperienceExampleLifecycle,
    ControllerExperienceInventory, ControllerExperienceOutcome, ControllerExperienceProvenance,
    ControllerExperienceQuality, ControllerExperienceVerificationBasis,
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
    let project_id = database.create_project("experience inventory").unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&database_path, directory.path(), &registry_path).unwrap();
    (directory, app, project_id, registry_path)
}

fn draft(
    project_id: i64,
    capability: &str,
    basis: ControllerExperienceVerificationBasis,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceExampleDraft {
    let correction = (outcome == ControllerExperienceOutcome::Corrected).then(|| {
        orc::controller_experience::ControllerExperienceCorrectionMetadata {
            original_output: serde_json::json!({"decision": "observed"}),
            operator_reference: "operator:inventory".into(),
            reason: "explicit inventory test correction".into(),
        }
    });
    ControllerExperienceExampleDraft {
        schema_version: orc::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
        capability: capability.into(),
        input: serde_json::json!({"project_id": project_id, "capability": capability}),
        accepted_output: if outcome == ControllerExperienceOutcome::Corrected {
            serde_json::json!({"decision": "accepted"})
        } else {
            serde_json::json!({"decision": "accepted", "index": project_id})
        },
        verification_basis: basis,
        provenance: ControllerExperienceProvenance::default(),
        correction,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit inventory fixture".into(),
        },
    }
}

fn insert(
    app: &OrcApp,
    project_id: i64,
    capability: &str,
    basis: ControllerExperienceVerificationBasis,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceExampleLifecycle {
    app.create_controller_experience_example(&draft(project_id, capability, basis, outcome))
        .unwrap()
        .lifecycle
}

fn expected_summary(capability: &str) -> ControllerExperienceCapabilitySummary {
    ControllerExperienceCapabilitySummary {
        capability: capability.into(),
        total: 0,
        active: 0,
        retired: 0,
        accepted: 0,
        corrected: 0,
        rejected: 0,
        operator_attestation: 0,
        explicit_correction: 0,
        external_evaluation: 0,
    }
}

#[test]
fn empty_dataset_returns_zero_inventory() {
    let (_directory, app, _project_id, _registry) = fixture();
    let inventory = app.controller_experience_inventory().unwrap();
    assert_eq!(
        inventory,
        ControllerExperienceInventory {
            schema_version: CONTROLLER_EXPERIENCE_INVENTORY_SCHEMA_VERSION,
            total: 0,
            active: 0,
            retired: 0,
            capabilities: Vec::new(),
        }
    );
    inventory.validate().unwrap();
}

#[test]
fn one_active_and_one_retired_row_have_exact_lifecycle_counts() {
    let (_directory, app, project_id, _registry) = fixture();
    insert(
        &app,
        project_id,
        "controller.active",
        ControllerExperienceVerificationBasis::OperatorAttestation,
        ControllerExperienceOutcome::Accepted,
    );
    let retired = app
        .create_controller_experience_example(&draft(
            project_id,
            "controller.retired",
            ControllerExperienceVerificationBasis::ExternalEvaluation,
            ControllerExperienceOutcome::Rejected,
        ))
        .unwrap();
    app.retire_controller_experience_example(retired.id)
        .unwrap();

    let inventory = app.controller_experience_inventory().unwrap();
    assert_eq!(inventory.total, 2);
    assert_eq!(inventory.active, 1);
    assert_eq!(inventory.retired, 1);
    assert_eq!(inventory.capabilities[0].capability, "controller.active");
    assert_eq!(inventory.capabilities[0].active, 1);
    assert_eq!(inventory.capabilities[0].retired, 0);
    assert_eq!(inventory.capabilities[1].capability, "controller.retired");
    assert_eq!(inventory.capabilities[1].active, 0);
    assert_eq!(inventory.capabilities[1].retired, 1);
    inventory.validate().unwrap();
}

#[test]
fn mixed_metadata_is_counted_once_and_sorted_by_exact_capability() {
    let (_directory, app, project_id, _registry) = fixture();
    insert(
        &app,
        project_id,
        "controller.zeta",
        ControllerExperienceVerificationBasis::ExternalEvaluation,
        ControllerExperienceOutcome::Rejected,
    );
    insert(
        &app,
        project_id,
        "controller.alpha",
        ControllerExperienceVerificationBasis::OperatorAttestation,
        ControllerExperienceOutcome::Accepted,
    );
    insert(
        &app,
        project_id,
        "controller.zeta",
        ControllerExperienceVerificationBasis::ExplicitCorrection,
        ControllerExperienceOutcome::Corrected,
    );
    insert(
        &app,
        project_id,
        "controller.beta",
        ControllerExperienceVerificationBasis::ExplicitCorrection,
        ControllerExperienceOutcome::Accepted,
    );
    insert(
        &app,
        project_id,
        "controller.beta",
        ControllerExperienceVerificationBasis::OperatorAttestation,
        ControllerExperienceOutcome::Corrected,
    );
    let retired = app
        .create_controller_experience_example(&draft(
            project_id,
            "controller.zeta",
            ControllerExperienceVerificationBasis::ExternalEvaluation,
            ControllerExperienceOutcome::Rejected,
        ))
        .unwrap();
    app.retire_controller_experience_example(retired.id)
        .unwrap();

    let inventory = app.controller_experience_inventory().unwrap();
    assert_eq!(inventory.total, 6);
    assert_eq!(inventory.active, 5);
    assert_eq!(inventory.retired, 1);
    assert_eq!(
        inventory
            .capabilities
            .iter()
            .map(|summary| summary.capability.as_str())
            .collect::<Vec<_>>(),
        vec!["controller.alpha", "controller.beta", "controller.zeta"]
    );

    let alpha = &inventory.capabilities[0];
    assert_eq!(alpha.total, 1);
    assert_eq!(alpha.active, 1);
    assert_eq!(alpha.retired, 0);
    assert_eq!(alpha.accepted, 1);
    assert_eq!(alpha.corrected, 0);
    assert_eq!(alpha.rejected, 0);
    assert_eq!(alpha.operator_attestation, 1);
    assert_eq!(alpha.explicit_correction, 0);
    assert_eq!(alpha.external_evaluation, 0);

    let beta = &inventory.capabilities[1];
    assert_eq!(beta.total, 2);
    assert_eq!(beta.active, 2);
    assert_eq!(beta.retired, 0);
    assert_eq!(beta.accepted, 1);
    assert_eq!(beta.corrected, 1);
    assert_eq!(beta.rejected, 0);
    assert_eq!(beta.operator_attestation, 1);
    assert_eq!(beta.explicit_correction, 1);
    assert_eq!(beta.external_evaluation, 0);

    let zeta = &inventory.capabilities[2];
    assert_eq!(zeta.total, 3);
    assert_eq!(zeta.active, 2);
    assert_eq!(zeta.retired, 1);
    assert_eq!(zeta.accepted, 0);
    assert_eq!(zeta.corrected, 1);
    assert_eq!(zeta.rejected, 2);
    assert_eq!(zeta.operator_attestation, 0);
    assert_eq!(zeta.explicit_correction, 1);
    assert_eq!(zeta.external_evaluation, 2);
    inventory.validate().unwrap();
}

#[test]
fn all_current_fixed_capabilities_are_preserved_without_aliasing() {
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
        insert(
            &app,
            project_id,
            capability,
            ControllerExperienceVerificationBasis::OperatorAttestation,
            ControllerExperienceOutcome::Accepted,
        );
    }

    let inventory = app.controller_experience_inventory().unwrap();
    let mut expected = capabilities.to_vec();
    expected.sort_unstable();
    assert_eq!(
        inventory
            .capabilities
            .iter()
            .map(|summary| summary.capability.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(inventory.total, capabilities.len() as u64);
    inventory.validate().unwrap();
}

#[test]
fn repeat_reads_are_identical_and_inventory_performs_no_writes() {
    let (_directory, app, project_id, registry) = fixture();
    insert(
        &app,
        project_id,
        "controller.repeat",
        ControllerExperienceVerificationBasis::OperatorAttestation,
        ControllerExperienceOutcome::Accepted,
    );
    let before = std::fs::read(&registry).unwrap();
    let first = app.controller_experience_inventory().unwrap();
    let second = app.controller_experience_inventory().unwrap();
    let after = std::fs::read(&registry).unwrap();
    assert_eq!(first, second);
    assert_eq!(before, after);
}

#[test]
fn retirement_changes_only_lifecycle_counts() {
    let (_directory, app, project_id, _registry) = fixture();
    let example = app
        .create_controller_experience_example(&draft(
            project_id,
            "controller.lifecycle",
            ControllerExperienceVerificationBasis::ExplicitCorrection,
            ControllerExperienceOutcome::Corrected,
        ))
        .unwrap();
    let before = app.controller_experience_inventory().unwrap();
    app.retire_controller_experience_example(example.id)
        .unwrap();
    let after = app.controller_experience_inventory().unwrap();
    assert_eq!(before.total, after.total);
    assert_eq!(before.active, 1);
    assert_eq!(after.active, 0);
    assert_eq!(before.retired, 0);
    assert_eq!(after.retired, 1);
    assert_eq!(
        before.capabilities[0].accepted,
        after.capabilities[0].accepted
    );
    assert_eq!(
        before.capabilities[0].corrected,
        after.capabilities[0].corrected
    );
    assert_eq!(
        before.capabilities[0].rejected,
        after.capabilities[0].rejected
    );
    assert_eq!(
        before.capabilities[0].operator_attestation,
        after.capabilities[0].operator_attestation
    );
    assert_eq!(
        before.capabilities[0].explicit_correction,
        after.capabilities[0].explicit_correction
    );
    assert_eq!(
        before.capabilities[0].external_evaluation,
        after.capabilities[0].external_evaluation
    );
}

#[test]
fn inventory_invariants_reject_tampered_aggregate_shapes() {
    let summary = expected_summary("controller.invalid");
    let valid = ControllerExperienceInventory {
        schema_version: CONTROLLER_EXPERIENCE_INVENTORY_SCHEMA_VERSION,
        total: 0,
        active: 0,
        retired: 0,
        capabilities: vec![summary],
    };
    valid.validate().unwrap();

    let mut invalid = valid.clone();
    invalid.total = 1;
    assert!(invalid.validate().is_err());
    let mut invalid = valid.clone();
    invalid.capabilities[0].accepted = 1;
    assert!(invalid.validate().is_err());
    let mut invalid = valid;
    invalid.capabilities[0].operator_attestation = 1;
    assert!(invalid.validate().is_err());
}

fn assert_corrupt_metadata_fails(update: impl FnOnce(&Connection)) {
    let (directory, app, project_id, registry) = fixture();
    app.create_controller_experience_example(&draft(
        project_id,
        "controller.corrupt",
        ControllerExperienceVerificationBasis::OperatorAttestation,
        ControllerExperienceOutcome::Accepted,
    ))
    .unwrap();
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
    assert!(reopened.controller_experience_inventory().is_err());
}

#[test]
fn malformed_persisted_metadata_fails_closed() {
    assert_corrupt_metadata_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET schema_version = ?1",
                params![999_i64],
            )
            .unwrap();
    });
    assert_corrupt_metadata_fails(|connection| {
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
    assert_corrupt_metadata_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET outcome = ?1",
                params!["unknown"],
            )
            .unwrap();
    });
    assert_corrupt_metadata_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET verification_basis = ?1",
                params!["automatic_success"],
            )
            .unwrap();
    });
    assert_corrupt_metadata_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET capability = ?1",
                params![""],
            )
            .unwrap();
    });
    assert_corrupt_metadata_fails(|connection| {
        connection
            .execute(
                "UPDATE controller_experience_examples SET capability = ?1",
                params!["x".repeat(129)],
            )
            .unwrap();
    });
}
