use orc::app::OrcApp;
use orc::controller_experience::{
    ControllerExperienceCorrectionMetadata, ControllerExperienceExampleQuery,
    ControllerExperienceLifecycleFilter, ControllerExperienceOutcome,
    ControllerExperienceProvenance, ControllerExperienceQuality,
    ControllerExperienceVerificationBasis, MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES,
};
use orc::controller_experience_memory_maintenance::{
    CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY,
    ControllerExperienceMemoryMaintenanceRequest,
};
use orc::controller_memory::{
    CONTROLLER_MEMORY_CONTEXT_VERSION, ControllerMemoryAuthority, ControllerMemoryContext,
    ControllerMemoryItem,
};
use orc::controller_memory_maintenance::{
    ControllerMemoryMaintenanceError, ControllerMemoryMaintenanceInput,
    ControllerMemoryMaintenanceRequest, ControllerMemoryMaintenanceResult,
    MAX_CONTROLLER_MEMORY_MAINTENANCE_INPUT_BYTES,
};
use orc::controller_memory_mutation::ControllerMemoryMutationIntent;
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
    MemoryRecord, MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

fn fixture() -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    let db_path = directory.path().join("orc.db");
    let registry_path = directory.path().join("global.db");
    let database = Database::init_with_registry(&db_path, &registry_path).unwrap();
    let project_id = database
        .create_project("memory maintenance curation")
        .unwrap();
    drop(database);
    let app = OrcApp::open_with_registry(&db_path, directory.path(), &registry_path).unwrap();
    (directory, app, project_id)
}

fn target(project_id: i64) -> MemoryRecord {
    MemoryRecord {
        id: MemoryId::Project { project_id, id: 1 },
        kind: MemoryKind::Project,
        scope: MemoryScope::Project { project_id },
        subject: "release-gate".into(),
        content: "Releases used to require manual approval.".into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Imported,
            source_reference: Some("history:release-gate".into()),
        },
        confidence: Some(0.7),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn memory_item(id: i64, content: &str) -> ControllerMemoryItem {
    ControllerMemoryItem {
        id: MemoryId::Global(id),
        kind: MemoryKind::User,
        scope: MemoryScope::Global,
        authority: ControllerMemoryAuthority::DurableUser,
        subject: "maintenance-guidance".into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("test:memory-maintenance-curation".into()),
        },
        confidence: Some(0.8),
        lifecycle: MemoryLifecycle::Active,
        supersedes: None,
    }
}

fn request_input(project_id: i64) -> ControllerMemoryMaintenanceRequest {
    ControllerMemoryMaintenanceRequest::new(
        target(project_id).id,
        vec!["The operator now requires two-person approval.".into()],
    )
}

fn input(project_id: i64) -> ControllerMemoryMaintenanceInput {
    ControllerMemoryMaintenanceInput::from_resolved_target(
        &request_input(project_id),
        target(project_id),
        ControllerMemoryContext {
            context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
            items: vec![memory_item(
                1,
                "Historical memory is advisory; the supplied target and facts are authoritative.",
            )],
        },
    )
}

fn replacement(input: &ControllerMemoryMaintenanceInput, content: &str) -> MemoryDraft {
    MemoryDraft {
        kind: input.target.kind,
        scope: input.target.scope.clone(),
        subject: input.target.subject.clone(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("operator:release-gate".into()),
        },
        confidence: Some(0.95),
    }
}

fn proposal(
    input: &ControllerMemoryMaintenanceInput,
    operation: &str,
    content: &str,
) -> ControllerMemoryMaintenanceResult {
    proposal_with_replacement(input, operation, replacement(input, content))
}

fn proposal_with_replacement(
    input: &ControllerMemoryMaintenanceInput,
    operation: &str,
    replacement: MemoryDraft,
) -> ControllerMemoryMaintenanceResult {
    let target = input.target.id.clone();
    let intent = match operation {
        "correct" => ControllerMemoryMutationIntent::Correct {
            target,
            replacement,
        },
        "supersede" => ControllerMemoryMutationIntent::Supersede {
            target,
            replacement,
        },
        "remove" => ControllerMemoryMutationIntent::Remove { target },
        _ => panic!("unsupported test operation"),
    };
    ControllerMemoryMaintenanceResult::ProposeMutation { intent }
}

fn curation_request(
    input: ControllerMemoryMaintenanceInput,
    observed: ControllerMemoryMaintenanceResult,
    accepted: ControllerMemoryMaintenanceResult,
    outcome: ControllerExperienceOutcome,
) -> ControllerExperienceMemoryMaintenanceRequest {
    ControllerExperienceMemoryMaintenanceRequest {
        input,
        observed,
        accepted,
        verification_basis: ControllerExperienceVerificationBasis::OperatorAttestation,
        provenance: ControllerExperienceProvenance::default(),
        correction: None,
        outcome,
        quality: ControllerExperienceQuality {
            score: 100,
            rationale: "explicit memory-maintenance curation evidence".into(),
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

fn assert_zero_rows(app: &OrcApp, request: &ControllerExperienceMemoryMaintenanceRequest) {
    assert!(
        app.create_controller_memory_maintenance_experience_example(request)
            .is_err()
    );
    assert!(all(app).is_empty());
}

#[test]
fn equal_keep_persists_one_exact_active_non_correction_example() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let expected_input = serde_json::to_value(&input).unwrap();
    let request = curation_request(
        input,
        ControllerMemoryMaintenanceResult::Keep,
        ControllerMemoryMaintenanceResult::Keep,
        ControllerExperienceOutcome::Accepted,
    );

    let stored = app
        .create_controller_memory_maintenance_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.accepted_output,
        serde_json::json!({"decision": "keep"})
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY
    );
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Accepted);
    assert!(stored.correction.is_none());
}

#[test]
fn equal_valid_correct_supersede_and_remove_persist_complete_results() {
    for operation in ["correct", "supersede", "remove"] {
        let (_directory, app, project_id) = fixture();
        let input = input(project_id);
        let output = proposal(&input, operation, "Releases require two-person approval.");
        let expected_input = serde_json::to_value(&input).unwrap();
        let expected_output = serde_json::to_value(&output).unwrap();
        let request = curation_request(
            input,
            output.clone(),
            output,
            ControllerExperienceOutcome::Accepted,
        );

        let stored = app
            .create_controller_memory_maintenance_experience_example(&request)
            .unwrap();
        assert_eq!(all(&app).len(), 1);
        assert_eq!(stored.input, expected_input);
        assert_eq!(stored.accepted_output, expected_output);
        assert!(stored.correction.is_none());
    }
}

#[test]
fn corrected_keep_to_proposal_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let observed = ControllerMemoryMaintenanceResult::Keep;
    let accepted = proposal(&input, "correct", "Releases require two-person approval.");
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = curation_request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:maintenance-1".into(),
        reason: "The target was explicitly corrected.".into(),
    });

    let stored = app
        .create_controller_memory_maintenance_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
    assert_eq!(stored.outcome, ControllerExperienceOutcome::Corrected);
}

#[test]
fn corrected_proposal_to_keep_preserves_exact_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let observed = proposal(&input, "remove", "unused");
    let accepted = ControllerMemoryMaintenanceResult::Keep;
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = curation_request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:maintenance-2".into(),
        reason: "The target remains valid.".into(),
    });

    let stored = app
        .create_controller_memory_maintenance_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
}

#[test]
fn corrected_between_two_valid_proposals_preserves_complete_outputs() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let observed = proposal(&input, "correct", "The corrected release rule.");
    let accepted = proposal(&input, "supersede", "The newer release rule.");
    let observed_output = serde_json::to_value(&observed).unwrap();
    let accepted_output = serde_json::to_value(&accepted).unwrap();
    let mut request = curation_request(
        input,
        observed,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    request.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: observed_output.clone(),
        operator_reference: "operator:maintenance-3".into(),
        reason: "The accepted maintenance operation changed explicitly.".into(),
    });

    let stored = app
        .create_controller_memory_maintenance_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
    assert_eq!(stored.accepted_output, accepted_output);
    assert_eq!(stored.correction.unwrap().original_output, observed_output);
}

#[test]
fn exact_full_input_projection_preserves_request_target_facts_record_and_memory() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let expected_input = serde_json::to_value(&input).unwrap();
    let request = curation_request(
        input,
        ControllerMemoryMaintenanceResult::Keep,
        ControllerMemoryMaintenanceResult::Keep,
        ControllerExperienceOutcome::Accepted,
    );
    let stored = app
        .create_controller_memory_maintenance_experience_example(&request)
        .unwrap();

    assert_eq!(stored.input, expected_input);
    assert_eq!(
        stored.input["current_request"]["current_facts"][0],
        "The operator now requires two-person approval."
    );
    assert_eq!(stored.input["target"]["subject"], "release-gate");
    assert_eq!(
        stored.input["memory"]["items"][0]["content"],
        "Historical memory is advisory; the supplied target and facts are authoritative."
    );
    assert_eq!(
        stored.capability,
        CONTROLLER_MEMORY_MAINTENANCE_EXPERIENCE_CAPABILITY
    );
}

#[test]
fn malformed_input_and_production_target_validation_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let valid_input = input(project_id);
    let keep = ControllerMemoryMaintenanceResult::Keep;

    let mut malformed_input = valid_input.clone();
    malformed_input.current_request.packet_version = 0;
    assert_zero_rows(
        &app,
        &curation_request(
            malformed_input,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut mismatched = valid_input.clone();
    mismatched.current_request.target = MemoryId::Project { project_id, id: 2 };
    assert_zero_rows(
        &app,
        &curation_request(
            mismatched,
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut inactive = valid_input;
    inactive.target.lifecycle = MemoryLifecycle::Removed;
    assert_zero_rows(
        &app,
        &curation_request(
            inactive,
            keep.clone(),
            keep,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn genuine_production_input_bound_rejection_writes_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let mut large = input(project_id);
    large.current_request.current_facts = (0..16)
        .map(|index| format!("fact-{index}-{}", "f".repeat(1_900)))
        .collect();
    large.target.content = "t".repeat(16_000);
    large.memory = ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: (1..=8)
            .map(|id| memory_item(id, &"m".repeat(3_790)))
            .collect(),
    };
    assert!(large.current_request.validate().is_ok());
    assert!(large.target.validate().is_ok());
    assert!(large.memory.validate().is_ok());
    let error = large.validate().unwrap_err();
    assert!(matches!(
        error,
        ControllerMemoryMaintenanceError::InputTooLarge {
            max: MAX_CONTROLLER_MEMORY_MAINTENANCE_INPUT_BYTES,
            ..
        }
    ));
    assert_zero_rows(
        &app,
        &curation_request(
            large,
            ControllerMemoryMaintenanceResult::Keep,
            ControllerMemoryMaintenanceResult::Keep,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn invalid_observed_and_accepted_maintenance_results_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let valid_input = input(project_id);
    let keep = ControllerMemoryMaintenanceResult::Keep;

    let create = ControllerMemoryMaintenanceResult::ProposeMutation {
        intent: ControllerMemoryMutationIntent::Create {
            draft: replacement(&valid_input, "A create is not maintenance."),
        },
    };
    assert_zero_rows(
        &app,
        &curation_request(
            valid_input.clone(),
            create.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &curation_request(
            valid_input.clone(),
            keep.clone(),
            create,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let retarget = ControllerMemoryMaintenanceResult::ProposeMutation {
        intent: ControllerMemoryMutationIntent::Remove {
            target: MemoryId::Project { project_id, id: 2 },
        },
    };
    assert_zero_rows(
        &app,
        &curation_request(
            valid_input.clone(),
            retarget.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &curation_request(
            valid_input.clone(),
            keep.clone(),
            retarget,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    for operation in ["correct", "supersede"] {
        for field in ["kind", "scope", "subject"] {
            let mut changed = replacement(&valid_input, "changed replacement");
            match field {
                "kind" => changed.kind = MemoryKind::Episodic,
                "scope" => {
                    changed.scope = MemoryScope::Project {
                        project_id: project_id + 1,
                    }
                }
                "subject" => changed.subject = "different-subject".into(),
                _ => unreachable!(),
            }
            let observed = proposal_with_replacement(&valid_input, operation, changed.clone());
            assert_zero_rows(
                &app,
                &curation_request(
                    valid_input.clone(),
                    observed,
                    keep.clone(),
                    ControllerExperienceOutcome::Accepted,
                ),
            );
            let accepted = proposal_with_replacement(&valid_input, operation, changed);
            assert_zero_rows(
                &app,
                &curation_request(
                    valid_input.clone(),
                    keep.clone(),
                    accepted,
                    ControllerExperienceOutcome::Accepted,
                ),
            );
        }
    }

    let invalid_replacement = ControllerMemoryMaintenanceResult::ProposeMutation {
        intent: ControllerMemoryMutationIntent::Correct {
            target: valid_input.target.id.clone(),
            replacement: MemoryDraft {
                content: String::new(),
                ..replacement(&valid_input, "invalid replacement")
            },
        },
    };
    assert_zero_rows(
        &app,
        &curation_request(
            valid_input,
            invalid_replacement,
            keep,
            ControllerExperienceOutcome::Accepted,
        ),
    );
}

#[test]
fn invalid_m08_metadata_and_correction_states_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let keep = ControllerMemoryMaintenanceResult::Keep;
    let accepted = proposal(&input, "remove", "unused");

    let mut invalid_quality = curation_request(
        input.clone(),
        keep.clone(),
        keep.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_quality.quality.score = 101;
    assert_zero_rows(&app, &invalid_quality);

    let mut invalid_provenance = curation_request(
        input.clone(),
        keep.clone(),
        keep.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    invalid_provenance.provenance.project_id = Some(0);
    assert_zero_rows(&app, &invalid_provenance);

    let mut equal_with_correction = curation_request(
        input.clone(),
        keep.clone(),
        keep.clone(),
        ControllerExperienceOutcome::Accepted,
    );
    equal_with_correction.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::to_value(&keep).unwrap(),
        operator_reference: "operator:maintenance".into(),
        reason: "Equal outputs cannot be corrected.".into(),
    });
    assert_zero_rows(&app, &equal_with_correction);

    assert_zero_rows(
        &app,
        &curation_request(
            input.clone(),
            keep.clone(),
            keep.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    assert_zero_rows(
        &app,
        &curation_request(
            input.clone(),
            keep.clone(),
            accepted.clone(),
            ControllerExperienceOutcome::Accepted,
        ),
    );
    assert_zero_rows(
        &app,
        &curation_request(
            input.clone(),
            keep.clone(),
            accepted.clone(),
            ControllerExperienceOutcome::Corrected,
        ),
    );

    let mut wrong_original = curation_request(
        input,
        keep,
        accepted,
        ControllerExperienceOutcome::Corrected,
    );
    wrong_original.correction = Some(ControllerExperienceCorrectionMetadata {
        original_output: serde_json::json!({"decision": "not-observed"}),
        operator_reference: "operator:maintenance".into(),
        reason: "The observed output must be preserved exactly.".into(),
    });
    assert_zero_rows(&app, &wrong_original);
}

#[test]
fn distinct_m08_payload_and_complete_record_bounds_write_zero_rows() {
    let (_directory, app, project_id) = fixture();
    let mut large_input = input(project_id);
    large_input.memory = ControllerMemoryContext {
        context_version: CONTROLLER_MEMORY_CONTEXT_VERSION,
        items: (1..=8)
            .map(|id| memory_item(id, &"m".repeat(2_500)))
            .collect(),
    };
    assert!(large_input.validate().is_ok());
    assert!(
        serde_json::to_vec(&large_input).unwrap().len() > MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
    );
    assert_zero_rows(
        &app,
        &curation_request(
            large_input,
            ControllerMemoryMaintenanceResult::Keep,
            ControllerMemoryMaintenanceResult::Keep,
            ControllerExperienceOutcome::Accepted,
        ),
    );

    let mut large_target_input = input(project_id);
    large_target_input.target.content = "t".repeat(15_000);
    let accepted = proposal(&large_target_input, "correct", &"r".repeat(15_000));
    let mut record_bound_request = curation_request(
        large_target_input,
        accepted.clone(),
        accepted,
        ControllerExperienceOutcome::Accepted,
    );
    record_bound_request.provenance.task_id = Some("t".repeat(256));
    record_bound_request.provenance.source_reference = Some("s".repeat(256));
    record_bound_request.quality.rationale = "q".repeat(1024);
    assert!(record_bound_request.input.validate().is_ok());
    assert!(
        serde_json::to_vec(&record_bound_request.input)
            .unwrap()
            .len()
            <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
    );
    assert!(
        serde_json::to_vec(&record_bound_request.accepted)
            .unwrap()
            .len()
            <= MAX_CONTROLLER_EXPERIENCE_PAYLOAD_BYTES
    );
    assert!(
        record_bound_request
            .accepted
            .validate(&record_bound_request.input)
            .is_ok()
    );
    assert!(
        record_bound_request
            .into_example_draft()
            .unwrap()
            .validate()
            .is_err()
    );
    assert_zero_rows(&app, &record_bound_request);
}

#[test]
fn successful_call_creates_exactly_one_row() {
    let (_directory, app, project_id) = fixture();
    let input = input(project_id);
    let output = proposal(&input, "remove", "unused");
    let request = curation_request(
        input,
        output.clone(),
        output,
        ControllerExperienceOutcome::Accepted,
    );
    app.create_controller_memory_maintenance_experience_example(&request)
        .unwrap();
    assert_eq!(all(&app).len(), 1);
}
