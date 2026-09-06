use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExample,
    ControllerExperienceExampleDraft, ControllerExperienceExampleLifecycle,
    ControllerExperienceExampleQuery, ControllerExperienceLifecycleFilter,
    ControllerExperienceOutcome, ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_CAPABILITY_BYTES,
    MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::memory::MemoryId;
use orc::storage::Database;
use tempfile::TempDir;

fn open_app(
    directory: &TempDir,
    name: &str,
    suffix: &str,
    registry: &std::path::Path,
) -> (OrcApp, i64) {
    let project_path = directory.path().join(format!("{suffix}.db"));
    let database = Database::init_with_registry(&project_path, registry).unwrap();
    let project_id = database.create_project(name).unwrap();
    drop(database);
    (
        OrcApp::open_with_registry(&project_path, directory.path(), registry).unwrap(),
        project_id,
    )
}

fn fixture() -> (TempDir, OrcApp, i64, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let registry = directory.path().join(".orc/global.db");
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let (app, project_id) = open_app(&directory, "experience project", "project", &registry);
    (directory, app, project_id, registry)
}

fn draft(project_id: i64, capability: &str) -> ControllerExperienceExampleDraft {
    ControllerExperienceExampleDraft {
        schema_version: orc::controller_experience::CONTROLLER_EXPERIENCE_EXAMPLE_SCHEMA_VERSION,
        capability: capability.into(),
        input: serde_json::json!({"request": "explicit operator evidence"}),
        accepted_output: serde_json::json!({"decision": "accepted"}),
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance {
            project_id: Some(project_id),
            task_id: Some("T-0001".into()),
            run_id: Some(1),
            plan_id: Some(2),
            review_id: Some(3),
            memory_id: Some(MemoryId::Project { project_id, id: 4 }),
            source_reference: Some("operator:curated".into()),
        },
        correction: None,
        outcome: ControllerExperienceOutcome::Accepted,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicitly verified by the operator".into(),
        },
    }
}

fn all_query() -> ControllerExperienceExampleQuery {
    ControllerExperienceExampleQuery {
        capability: None,
        lifecycle: ControllerExperienceLifecycleFilter::All,
        limit: 128,
        offset: 0,
    }
}

fn assert_example_round_trip(app: &OrcApp, example: &ControllerExperienceExample) {
    assert_eq!(
        app.get_controller_experience_example(example.id).unwrap(),
        Some(example.clone())
    );
    assert_eq!(
        example.lifecycle,
        ControllerExperienceExampleLifecycle::Active
    );
}

#[test]
fn create_get_round_trip_and_deterministic_bounded_listing() {
    let (_directory, app, project_id, _registry) = fixture();
    let first = app
        .create_controller_experience_example(&draft(project_id, "review"))
        .unwrap();
    let second = app
        .create_controller_experience_example(&draft(project_id, "dispatch"))
        .unwrap();
    assert_example_round_trip(&app, &first);
    assert_example_round_trip(&app, &second);

    let listed = app
        .list_controller_experience_examples(&all_query())
        .unwrap();
    assert_eq!(listed, vec![first.clone(), second.clone()]);

    let page = ControllerExperienceExampleQuery {
        capability: None,
        lifecycle: ControllerExperienceLifecycleFilter::All,
        limit: 1,
        offset: 1,
    };
    assert_eq!(
        app.list_controller_experience_examples(&page).unwrap(),
        vec![second]
    );
    let filtered = ControllerExperienceExampleQuery {
        capability: Some("review".into()),
        lifecycle: ControllerExperienceLifecycleFilter::Active,
        limit: 4,
        offset: 0,
    };
    assert_eq!(
        app.list_controller_experience_examples(&filtered).unwrap(),
        vec![first]
    );
}

#[test]
fn active_and_retired_visibility_preserves_history_and_provenance() {
    let (_directory, app, project_id, _registry) = fixture();
    let example = app
        .create_controller_experience_example(&draft(project_id, "retire"))
        .unwrap();
    let retired = app
        .retire_controller_experience_example(example.id)
        .unwrap();
    assert_eq!(
        retired.lifecycle,
        ControllerExperienceExampleLifecycle::Retired
    );
    assert_eq!(retired.provenance, example.provenance);
    assert!(
        app.list_controller_experience_examples(&ControllerExperienceExampleQuery::active(10, 0))
            .unwrap()
            .is_empty()
    );
    let historical = ControllerExperienceExampleQuery {
        capability: None,
        lifecycle: ControllerExperienceLifecycleFilter::Retired,
        limit: 10,
        offset: 0,
    };
    assert_eq!(
        app.list_controller_experience_examples(&historical)
            .unwrap(),
        vec![retired]
    );
    assert_eq!(
        app.get_controller_experience_example(example.id)
            .unwrap()
            .unwrap()
            .id,
        example.id
    );
}

#[test]
fn examples_are_global_across_projects_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let registry = directory.path().join(".orc/global.db");
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let (first, project_a) = open_app(&directory, "project a", "a", &registry);
    let (second, project_b) = open_app(&directory, "project b", "b", &registry);
    let example = first
        .create_controller_experience_example(&draft(project_a, "shared"))
        .unwrap();

    assert_eq!(
        second
            .get_controller_experience_example(example.id)
            .unwrap(),
        Some(example.clone())
    );
    assert_eq!(
        second
            .list_controller_experience_examples(&all_query())
            .unwrap(),
        vec![example.clone()]
    );
    assert!(project_a > 0 && project_b > 0);
    drop(first);
    let reopened =
        OrcApp::open_with_registry(directory.path().join("a.db"), directory.path(), &registry)
            .unwrap();
    assert_eq!(
        reopened
            .get_controller_experience_example(example.id)
            .unwrap(),
        Some(example)
    );
}

#[test]
fn invalid_examples_are_rejected_before_any_row_is_written() {
    let (_directory, app, project_id, _registry) = fixture();
    let before = app
        .list_controller_experience_examples(&all_query())
        .unwrap();
    let mut cases = Vec::new();

    let mut unsupported_version = draft(project_id, "version");
    unsupported_version.schema_version = 99;
    cases.push(unsupported_version);

    let mut empty_capability = draft(project_id, "valid");
    empty_capability.capability = " ".into();
    cases.push(empty_capability);

    let mut oversized_capability = draft(project_id, "valid");
    oversized_capability.capability = "x".repeat(MAX_CONTROLLER_EXPERIENCE_CAPABILITY_BYTES + 1);
    cases.push(oversized_capability);

    let mut empty_input = draft(project_id, "valid");
    empty_input.input = serde_json::Value::Null;
    cases.push(empty_input);

    let mut oversized_input = draft(project_id, "valid");
    oversized_input.input =
        serde_json::Value::String("x".repeat(MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES));
    cases.push(oversized_input);

    let mut oversized_output = draft(project_id, "valid");
    oversized_output.accepted_output =
        serde_json::Value::String("x".repeat(MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES));
    cases.push(oversized_output);

    let mut oversized_record = draft(project_id, "valid");
    oversized_record.input = serde_json::Value::String("x".repeat(16_200));
    oversized_record.accepted_output = serde_json::Value::String("y".repeat(16_200));
    cases.push(oversized_record);

    let mut invalid_project = draft(project_id, "valid");
    invalid_project.provenance.project_id = Some(0);
    cases.push(invalid_project);

    let mut invalid_reference = draft(project_id, "valid");
    invalid_reference.provenance.task_id = Some(" ".into());
    cases.push(invalid_reference);

    let mut corrected_without_metadata = draft(project_id, "valid");
    corrected_without_metadata.outcome = ControllerExperienceOutcome::Corrected;
    cases.push(corrected_without_metadata);

    let mut correction_without_changed_output = draft(project_id, "valid");
    correction_without_changed_output.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: correction_without_changed_output.accepted_output.clone(),
        operator_reference: "operator:1".into(),
        reason: "correction".into(),
    });
    cases.push(correction_without_changed_output);

    let mut invalid_quality = draft(project_id, "valid");
    invalid_quality.quality.score = 101;
    cases.push(invalid_quality);

    for (index, case) in cases.into_iter().enumerate() {
        assert!(
            app.create_controller_experience_example(&case).is_err(),
            "invalid case {index} unexpectedly persisted"
        );
        assert_eq!(
            app.list_controller_experience_examples(&all_query())
                .unwrap(),
            before
        );
    }
}

#[test]
fn invalid_verification_basis_and_query_bounds_fail_closed() {
    let (_directory, app, project_id, _registry) = fixture();
    let value = serde_json::to_value(draft(project_id, "basis")).unwrap();
    let mut invalid = value;
    invalid["verification_basis"] = serde_json::json!("automatic_success");
    assert!(serde_json::from_value::<ControllerExperienceExampleDraft>(invalid).is_err());

    for query in [
        ControllerExperienceExampleQuery::active(0, 0),
        ControllerExperienceExampleQuery::active(129, 0),
        ControllerExperienceExampleQuery::active(1, 1_000_001),
    ] {
        assert!(app.list_controller_experience_examples(&query).is_err());
    }
}

#[test]
fn correction_metadata_is_persisted_only_for_explicit_corrected_outcome() {
    let (_directory, app, project_id, _registry) = fixture();
    let mut corrected = draft(project_id, "correction");
    corrected.accepted_output = serde_json::json!({"decision": "corrected"});
    corrected.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"decision": "wrong"}),
        operator_reference: "operator:correction-1".into(),
        reason: "operator supplied the canonical correction".into(),
    });
    corrected.outcome = ControllerExperienceOutcome::Corrected;
    let example = app
        .create_controller_experience_example(&corrected)
        .unwrap();
    assert_eq!(example.correction, corrected.correction);
    assert_eq!(example.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn runtime_experience_memory_remains_a_separate_global_memory_surface() {
    let (_directory, app, _project_id, _registry) = fixture();
    let memory = app
        .memories()
        .unwrap()
        .create(&orc::memory::MemoryDraft {
            kind: orc::memory::MemoryKind::Experience,
            scope: orc::memory::MemoryScope::Global,
            subject: "runtime experience".into(),
            content: "runtime memory remains unchanged".into(),
            provenance: orc::memory::MemoryProvenance {
                kind: orc::memory::MemoryProvenanceKind::Imported,
                source_reference: Some("memory-regression".into()),
            },
            confidence: Some(0.7),
        })
        .unwrap();
    assert_eq!(memory.kind, orc::memory::MemoryKind::Experience);
    assert!(
        app.list_controller_experience_examples(&all_query())
            .unwrap()
            .is_empty()
    );
}
