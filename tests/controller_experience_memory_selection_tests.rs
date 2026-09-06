use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::controller_experience_memory_selection::{
    CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY, ControllerExperienceMemorySelectionRequest,
};
use orc::controller_memory_selection::{
    ControllerMemorySelectionCandidate, ControllerMemorySelectionError,
    ControllerMemorySelectionInput, ControllerMemorySelectionRequest,
    ControllerMemorySelectionResult, MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATE_BYTES,
    MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES, MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES,
};
use orc::memory::{
    MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = database
        .create_project("memory selection curation")
        .unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app, project_id)
}

fn candidate(
    project_id: i64,
    id: i64,
    kind: MemoryKind,
    subject: &str,
    content: &str,
) -> ControllerMemorySelectionCandidate {
    ControllerMemorySelectionCandidate {
        id: MemoryId::Project { project_id, id },
        kind,
        scope: MemoryScope::Project { project_id },
        lifecycle: MemoryLifecycle::Active,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::ProjectFact,
            source_reference: Some(format!("project:{project_id}:memory:{id}")),
        },
        confidence: Some(0.8),
    }
}

fn input(
    project_id: i64,
    candidates: Vec<ControllerMemorySelectionCandidate>,
    current_facts: Vec<String>,
    eligible_candidate_count: usize,
) -> ControllerMemorySelectionInput {
    let selected_candidate_count = candidates.len();
    ControllerMemorySelectionInput {
        current_project_id: project_id,
        current_request: ControllerMemorySelectionRequest::new(current_facts),
        candidates,
        eligible_candidate_count,
        selected_candidate_count,
        omitted_candidate_count: eligible_candidate_count - selected_candidate_count,
    }
}

fn valid_input(project_id: i64) -> ControllerMemorySelectionInput {
    input(
        project_id,
        vec![
            candidate(project_id, 1, MemoryKind::Project, "alpha", "Alpha memory."),
            candidate(project_id, 2, MemoryKind::Episodic, "beta", "Beta memory."),
        ],
        vec!["The operator explicitly identified beta for maintenance.".into()],
        2,
    )
}

fn select(input: &ControllerMemorySelectionInput, id: i64) -> ControllerMemorySelectionResult {
    ControllerMemorySelectionResult::SelectTarget {
        target: MemoryId::Project {
            project_id: input.current_project_id,
            id,
        },
    }
}

fn request(
    input: ControllerMemorySelectionInput,
    observed: ControllerMemorySelectionResult,
    accepted: ControllerMemorySelectionResult,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceMemorySelectionRequest {
    ControllerExperienceMemorySelectionRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit memory-selection curation evidence".into(),
        },
    }
}

fn all(app: &OrcApp) -> Vec<orc::controller_experience::ControllerExperienceExample> {
    app.list_controller_experience_examples(&ControllerExperienceExampleQuery {
        capability: None,
        lifecycle: ControllerExperienceLifecycleFilter::All,
        limit: 128,
        offset: 0,
    })
    .unwrap()
}

fn assert_zero_rows(app: &OrcApp, request: &ControllerExperienceMemorySelectionRequest) {
    assert!(
        app.create_controller_memory_selection_experience_example(request)
            .is_err()
    );
    assert!(all(app).is_empty());
}

#[test]
fn equal_no_target_persists_one_exact_active_non_correction_example() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let expected_input = serde_json::to_value(&input).unwrap();
    let request = request(
        input,
        ControllerMemorySelectionResult::NoTarget,
        ControllerMemorySelectionResult::NoTarget,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_memory_selection_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::json!({"decision": "no_target"})
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_MEMORY_SELECTION_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn equal_select_target_persists_exact_complete_result() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let output = select(&input, 2);
    let expected_input = serde_json::to_value(&input).unwrap();
    let expected_output = serde_json::to_value(&output).unwrap();
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_memory_selection_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(stored.accepted_output, expected_output);
    assert_eq!(stored.accepted_output["decision"], "select_target");
    assert_eq!(stored.accepted_output["target"]["Project"]["id"], 2);
    assert!(stored.correction.is_none());
}

#[test]
fn corrected_no_target_to_valid_target_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let observed = ControllerMemorySelectionResult::NoTarget;
    let accepted = select(&input, 2);
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:selection-1".into(),
        reason: "The supplied beta candidate was explicitly selected.".into(),
    });

    let stored = app
        .create_controller_memory_selection_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn corrected_valid_target_to_no_target_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let observed = select(&input, 1);
    let accepted = ControllerMemorySelectionResult::NoTarget;
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:selection-2".into(),
        reason: "The candidate was explicitly rejected for maintenance.".into(),
    });

    let stored = app
        .create_controller_memory_selection_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
}

#[test]
fn corrected_valid_target_a_to_valid_target_b_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let observed = select(&input, 1);
    let accepted = select(&input, 2);
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:selection-3".into(),
        reason: "The accepted candidate changed explicitly.".into(),
    });

    let stored = app
        .create_controller_memory_selection_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
}

#[test]
fn exact_full_input_projection_preserves_candidate_order_and_metadata() {
    let (_directory, app, project_id) = fixture();
    let input = input(
        project_id,
        vec![
            candidate(project_id, 2, MemoryKind::Episodic, "second", "Second."),
            candidate(project_id, 1, MemoryKind::Project, "first", "First."),
        ],
        vec!["Fact one.".into(), "Fact two.".into()],
        3,
    );
    let expected_input = serde_json::to_value(&input).unwrap();
    let output = select(&input, 1);
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_memory_selection_experience_example(&request)
        .unwrap();

    assert_eq!(stored.input, expected_input);
    assert_eq!(stored.input["current_project_id"], project_id);
    assert_eq!(
        stored.input["current_request"]["current_facts"][0],
        "Fact one."
    );
    assert_eq!(
        stored.input["current_request"]["current_facts"][1],
        "Fact two."
    );
    assert_eq!(stored.input["candidates"][0]["subject"], "second");
    assert_eq!(stored.input["candidates"][1]["subject"], "first");
    assert_eq!(stored.input["eligible_candidate_count"], 3);
    assert_eq!(stored.input["selected_candidate_count"], 2);
    assert_eq!(stored.input["omitted_candidate_count"], 1);
}

#[test]
fn invalid_input_metadata_and_candidate_contracts_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let valid = valid_input(project_id);
    let keep = ControllerMemorySelectionResult::NoTarget;

    let mut invalid_project = valid.clone();
    invalid_project.current_project_id = 0;
    assert_zero_rows(
        &app,
        &request(
            invalid_project,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_facts = valid.clone();
    invalid_facts.current_request.packet_version = 0;
    assert_zero_rows(
        &app,
        &request(
            invalid_facts,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut empty_fact = valid.clone();
    empty_fact.current_request.current_facts = vec![String::new()];
    assert_zero_rows(
        &app,
        &request(
            empty_fact,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut inconsistent = valid.clone();
    inconsistent.omitted_candidate_count = 1;
    assert_zero_rows(
        &app,
        &request(
            inconsistent,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut duplicate = valid.clone();
    duplicate.candidates[1].id = duplicate.candidates[0].id.clone();
    assert_zero_rows(
        &app,
        &request(
            duplicate,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut inactive = valid.clone();
    inactive.candidates[0].lifecycle = MemoryLifecycle::Removed;
    assert_zero_rows(
        &app,
        &request(
            inactive,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    for invalid_candidate in [
        {
            let mut value = valid.candidates[0].clone();
            value.kind = MemoryKind::User;
            value.id = MemoryId::Global(1);
            value.scope = MemoryScope::Global;
            value
        },
        {
            let mut value = valid.candidates[0].clone();
            value.kind = MemoryKind::Experience;
            value.id = MemoryId::Global(2);
            value.scope = MemoryScope::Global;
            value
        },
        {
            let mut value = valid.candidates[0].clone();
            value.id = MemoryId::Project {
                project_id: project_id + 1,
                id: 1,
            };
            value.scope = MemoryScope::Project {
                project_id: project_id + 1,
            };
            value
        },
    ] {
        let mut invalid = valid.clone();
        invalid.candidates[0] = invalid_candidate;
        assert_zero_rows(
            &app,
            &request(
                invalid,
                keep.clone(),
                keep.clone(),
                ControllerExperienceOutcome::Accepted,
            ),
        );
    }

    let mut invalid_provenance = valid.clone();
    invalid_provenance.candidates[0].provenance.source_reference = Some(String::new());
    assert_zero_rows(
        &app,
        &request(
            invalid_provenance,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut invalid_confidence = valid.clone();
    invalid_confidence.candidates[0].confidence = Some(f64::NAN);
    assert_zero_rows(
        &app,
        &request(
            invalid_confidence,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut too_many = valid.clone();
    too_many.candidates = (1..=(MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATES as i64 + 1))
        .map(|id| candidate(project_id, id, MemoryKind::Project, "many", "Many."))
        .collect();
    too_many.selected_candidate_count = too_many.candidates.len();
    too_many.eligible_candidate_count = too_many.candidates.len();
    too_many.omitted_candidate_count = 0;
    assert_zero_rows(
        &app,
        &request(
            too_many,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut too_large = valid;
    too_large.candidates[0].content = "c".repeat(MAX_CONTROLLER_MEMORY_SELECTION_CANDIDATE_BYTES);
    assert_zero_rows(
        &app,
        &request(
            too_large,
            keep.clone(),
            keep,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn genuine_production_input_bound_rejection_writes_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let large_candidates = (1..=5)
        .map(|id| {
            candidate(
                project_id,
                id,
                MemoryKind::Project,
                "large-candidate",
                &"c".repeat(7_000),
            )
        })
        .collect();
    let large = input(project_id, large_candidates, vec!["fact".into()], 5);
    assert!(large.validate().is_err());
    let error = large.validate().unwrap_err();
    assert!(matches!(
        error,
        ControllerMemorySelectionError::InputTooLarge {
            max: MAX_CONTROLLER_MEMORY_SELECTION_INPUT_BYTES,
            ..
        }
    ));
    assert_zero_rows(
        &app,
        &request(
            large,
            ControllerMemorySelectionResult::NoTarget,
            ControllerMemorySelectionResult::NoTarget,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn observed_and_accepted_targets_must_be_supplied_candidates() {
    let (_directory, app, project_id) = fixture();
    let valid = valid_input(project_id);
    let absent = ControllerMemorySelectionResult::SelectTarget {
        target: MemoryId::Project { project_id, id: 99 },
    };
    assert_zero_rows(
        &app,
        &request(
            valid.clone(),
            absent.clone(),
            ControllerMemorySelectionResult::NoTarget,
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &request(
            valid,
            ControllerMemorySelectionResult::NoTarget,
            absent,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn invalid_m08_metadata_and_correction_states_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let no_target = ControllerMemorySelectionResult::NoTarget;
    let selected = select(&input, 1);

    let mut invalid_quality = request(
        input.clone(),
        no_target.clone(),
        no_target.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert_zero_rows(&app, &invalid_quality);

    let mut invalid_provenance = request(
        input.clone(),
        no_target.clone(),
        no_target.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_provenance.provenance.project_id = Some(0);
    assert_zero_rows(&app, &invalid_provenance);

    let mut equal_with_correction = request(
        input.clone(),
        no_target.clone(),
        no_target.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::to_value(&no_target).unwrap(),
        operator_reference: "operator:selection".into(),
        reason: "Equal outputs cannot be corrected.".into(),
    });
    assert_zero_rows(&app, &equal_with_correction);

    assert_zero_rows(
        &app,
        &request(
            input.clone(),
            no_target.clone(),
            no_target.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    assert_zero_rows(
        &app,
        &request(
            input.clone(),
            no_target.clone(),
            selected.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &request(
            input.clone(),
            no_target.clone(),
            selected.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    let mut wrong_original = request(
        input,
        no_target,
        selected,
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"decision": "not-observed"}),
        operator_reference: "operator:selection".into(),
        reason: "The observed output must be preserved exactly.".into(),
    });
    assert_zero_rows(&app, &wrong_original);
}

#[test]
fn distinct_m08_payload_bound_writes_zero_rows_with_valid_selection_contracts() {
    let (_directory, app, project_id) = fixture();
    let large_candidates = (1..=3)
        .map(|id| {
            candidate(
                project_id,
                id,
                MemoryKind::Project,
                "payload-candidate",
                &"p".repeat(5_500),
            )
        })
        .collect();
    let large = input(project_id, large_candidates, vec!["fact".into()], 3);
    assert!(large.validate().is_ok());
    assert!(serde_json::to_vec(&large).unwrap().len() > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES);
    let output = ControllerMemorySelectionResult::NoTarget;
    assert!(output.validate(&large).is_ok());
    assert!(serde_json::to_vec(&output).unwrap().len() <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES);
    assert_zero_rows(
        &app,
        &request(
            large,
            ControllerMemorySelectionResult::NoTarget,
            output,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn successful_call_creates_exactly_one_row() {
    let (_directory, app, project_id) = fixture();
    let input = valid_input(project_id);
    let output = select(&input, 2);
    let request = request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    app.create_controller_memory_selection_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
}
