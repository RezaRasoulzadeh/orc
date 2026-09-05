use orc::app::OrcApp;
use orc::controller_memory_maintenance_grant::{
    ControllerMemoryMaintenanceGrantError, ControllerMemoryMaintenanceGrantState,
    MAX_CONTROLLER_MEMORY_MAINTENANCE_ACTIONS,
};
use orc::controller_memory_mutation::{
    ControllerMemoryMutationAuthorizationRejection, ControllerMemoryMutationExecutionResult,
    ControllerMemoryMutationIntent, ControllerMemoryMutationOperation,
};
use orc::memory::{
    MemoryDraft, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryRecord,
    MemoryScope,
};
use orc::storage::Database;
use tempfile::TempDir;

fn open_app(name: &str) -> (TempDir, OrcApp, i64) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project_id = db.create_project(name).unwrap();
    drop(db);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    (directory, app, project_id)
}

fn draft(kind: MemoryKind, scope: MemoryScope, subject: &str, content: &str) -> MemoryDraft {
    MemoryDraft {
        kind,
        scope,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::ControllerApproved,
            source_reference: Some("controller:maintenance-grant-test".into()),
        },
        confidence: Some(0.8),
    }
}

fn target(app: &OrcApp, project_id: i64, kind: MemoryKind, subject: &str) -> MemoryRecord {
    app.memories()
        .unwrap()
        .create(&draft(
            kind,
            MemoryScope::Project { project_id },
            subject,
            "initial maintenance value",
        ))
        .unwrap()
}

fn proposal(
    app: &OrcApp,
    target: &MemoryRecord,
    operation: ControllerMemoryMutationOperation,
) -> orc::controller_memory_mutation::ControllerMemoryMutationProposal {
    let intent = match operation {
        ControllerMemoryMutationOperation::Correct => ControllerMemoryMutationIntent::Correct {
            target: target.id.clone(),
            replacement: draft(
                target.kind,
                target.scope.clone(),
                &target.subject,
                "corrected maintenance value",
            ),
        },
        ControllerMemoryMutationOperation::Supersede => ControllerMemoryMutationIntent::Supersede {
            target: target.id.clone(),
            replacement: draft(
                target.kind,
                target.scope.clone(),
                &target.subject,
                "superseding maintenance value",
            ),
        },
        ControllerMemoryMutationOperation::Remove => ControllerMemoryMutationIntent::Remove {
            target: target.id.clone(),
        },
        ControllerMemoryMutationOperation::Create => ControllerMemoryMutationIntent::Create {
            draft: draft(
                target.kind,
                target.scope.clone(),
                &target.subject,
                "create must be rejected",
            ),
        },
    };
    app.propose_controller_memory_mutation(intent).unwrap()
}

fn execute(
    app: &OrcApp,
    proposal: &orc::controller_memory_mutation::ControllerMemoryMutationProposal,
    authorization: orc::controller_memory_mutation::ControllerMemoryMutationAuthorization,
) -> ControllerMemoryMutationExecutionResult {
    app.execute_authorized_controller_memory_mutation(proposal, Some(authorization))
}

#[test]
fn one_unit_project_and_episodic_correct_supersede_remove_use_canonical_execution() {
    for kind in [MemoryKind::Project, MemoryKind::Episodic] {
        for operation in [
            ControllerMemoryMutationOperation::Correct,
            ControllerMemoryMutationOperation::Supersede,
            ControllerMemoryMutationOperation::Remove,
        ] {
            let (_directory, app, project_id) = open_app("maintenance eligible");
            let original = target(&app, project_id, kind, "eligible-target");
            let proposal = proposal(&app, &original, operation);
            let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
            let authorization = app
                .inspect_controller_memory_maintenance_grant(&grant, &proposal)
                .unwrap();
            assert_eq!(grant.remaining_actions().unwrap(), 0);
            assert_eq!(
                grant.state(),
                ControllerMemoryMaintenanceGrantState::Exhausted
            );
            assert!(matches!(
                execute(&app, &proposal, authorization),
                ControllerMemoryMutationExecutionResult::Mutated {
                    operation: actual,
                    ..
                } if actual == operation
            ));
            let expected_history = if operation == ControllerMemoryMutationOperation::Remove {
                1
            } else {
                2
            };
            assert_eq!(
                app.memories().unwrap().history(&original.id).unwrap().len(),
                expected_history
            );
        }
    }
}

#[test]
fn valid_multi_unit_grant_consumes_one_per_authorization_and_exhausts() {
    let (_directory, app, project_id) = open_app("maintenance budget");
    let grant = app.create_controller_memory_maintenance_grant(3).unwrap();
    let clone = grant.clone();
    let first = target(&app, project_id, MemoryKind::Project, "first");
    let second = target(&app, project_id, MemoryKind::Episodic, "second");
    let third = target(&app, project_id, MemoryKind::Project, "third");

    let first_proposal = proposal(&app, &first, ControllerMemoryMutationOperation::Correct);
    let first_authorization = app
        .inspect_controller_memory_maintenance_grant(&grant, &first_proposal)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 2);
    assert_eq!(clone.remaining_actions().unwrap(), 2);
    execute(&app, &first_proposal, first_authorization);

    let second_proposal = proposal(&app, &second, ControllerMemoryMutationOperation::Remove);
    let second_authorization = app
        .inspect_controller_memory_maintenance_grant(&clone, &second_proposal)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    execute(&app, &second_proposal, second_authorization);

    let third_proposal = proposal(&app, &third, ControllerMemoryMutationOperation::Supersede);
    let third_authorization = app
        .inspect_controller_memory_maintenance_grant(&grant, &third_proposal)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(
        clone.state(),
        ControllerMemoryMaintenanceGrantState::Exhausted
    );
    execute(&app, &third_proposal, third_authorization);

    let fourth = target(&app, project_id, MemoryKind::Project, "fourth");
    let fourth_proposal = proposal(&app, &fourth, ControllerMemoryMutationOperation::Remove);
    assert!(matches!(
        app.inspect_controller_memory_maintenance_grant(&clone, &fourth_proposal),
        Err(ControllerMemoryMaintenanceGrantError::Exhausted)
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 0);
}

#[test]
fn grant_budget_bounds_and_lifecycle_are_shared_across_clones() {
    let (_directory, app, project_id) = open_app("maintenance lifecycle");
    assert!(matches!(
        app.create_controller_memory_maintenance_grant(0),
        Err(ControllerMemoryMaintenanceGrantError::InvalidBudget)
    ));
    assert!(matches!(
        app.create_controller_memory_maintenance_grant(
            MAX_CONTROLLER_MEMORY_MAINTENANCE_ACTIONS + 1
        ),
        Err(ControllerMemoryMaintenanceGrantError::InvalidBudget)
    ));

    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let clone = grant.clone();
    grant.revoke().unwrap();
    assert_eq!(
        clone.state(),
        ControllerMemoryMaintenanceGrantState::Revoked
    );
    let original = target(&app, project_id, MemoryKind::Project, "revoked");
    let proposal = proposal(&app, &original, ControllerMemoryMutationOperation::Remove);
    assert!(matches!(
        app.inspect_controller_memory_maintenance_grant(&clone, &proposal),
        Err(ControllerMemoryMaintenanceGrantError::Revoked)
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 1);
}

#[test]
fn scope_mismatch_rejects_without_consumption() {
    let (_directory, app_a, _project_a) = open_app("maintenance project a");
    let grant = app_a.create_controller_memory_maintenance_grant(2).unwrap();

    let global_target = app_a
        .memories()
        .unwrap()
        .create(&draft(
            MemoryKind::User,
            MemoryScope::Global,
            "global-target",
            "global value",
        ))
        .unwrap();
    let global_proposal = app_a
        .propose_controller_memory_mutation(ControllerMemoryMutationIntent::Remove {
            target: global_target.id,
        })
        .unwrap();
    assert!(matches!(
        app_a.inspect_controller_memory_maintenance_grant(&grant, &global_proposal),
        Err(ControllerMemoryMaintenanceGrantError::InvalidScope)
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 2);
}

#[test]
fn create_user_and_experience_are_rejected_without_consumption() {
    let (_directory, app, project_id) = open_app("maintenance ineligible");
    let grant = app.create_controller_memory_maintenance_grant(4).unwrap();
    let project = target(&app, project_id, MemoryKind::Project, "create-target");
    let create_proposal = proposal(&app, &project, ControllerMemoryMutationOperation::Create);
    assert!(matches!(
        app.inspect_controller_memory_maintenance_grant(&grant, &create_proposal),
        Err(ControllerMemoryMaintenanceGrantError::UnsupportedOperation(
            ControllerMemoryMutationOperation::Create
        ))
    ));

    for kind in [MemoryKind::User, MemoryKind::Experience] {
        let global = app
            .memories()
            .unwrap()
            .create(&draft(
                kind,
                MemoryScope::Global,
                kind.as_str(),
                "global value",
            ))
            .unwrap();
        let global_proposal = app
            .propose_controller_memory_mutation(ControllerMemoryMutationIntent::Remove {
                target: global.id,
            })
            .unwrap();
        assert!(matches!(
            app.inspect_controller_memory_maintenance_grant(&grant, &global_proposal),
            Err(ControllerMemoryMaintenanceGrantError::InvalidScope)
        ));
    }
    assert_eq!(grant.remaining_actions().unwrap(), 4);
}

#[test]
fn exact_authorization_cannot_authorize_a_different_proposal() {
    let (_directory, app, project_id) = open_app("maintenance exact authorization");
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
    let first = target(&app, project_id, MemoryKind::Project, "first");
    let second = target(&app, project_id, MemoryKind::Project, "second");
    let first_proposal = proposal(&app, &first, ControllerMemoryMutationOperation::Remove);
    let second_proposal = proposal(&app, &second, ControllerMemoryMutationOperation::Remove);
    let authorization = app
        .inspect_controller_memory_maintenance_grant(&grant, &first_proposal)
        .unwrap();
    assert!(matches!(
        execute(&app, &second_proposal, authorization),
        ControllerMemoryMutationExecutionResult::AuthorizationRejected {
            reason: ControllerMemoryMutationAuthorizationRejection::NotAuthorizedForIntent,
            ..
        }
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert_eq!(
        app.memories()
            .unwrap()
            .get(&second.id)
            .unwrap()
            .unwrap()
            .lifecycle,
        MemoryLifecycle::Active
    );
}

#[test]
fn post_mint_failure_consumes_once_without_refund_and_history_stays_canonical() {
    let (directory, app, project_id) = open_app("maintenance failure");
    let path = directory.path().join(".orc/orc.db");
    let original = target(&app, project_id, MemoryKind::Project, "failure-target");
    let proposal = proposal(&app, &original, ControllerMemoryMutationOperation::Correct);
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    let authorization = app
        .inspect_controller_memory_maintenance_grant(&grant, &proposal)
        .unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_maintenance BEFORE UPDATE ON project_memories
             BEGIN SELECT RAISE(ABORT, 'maintenance execution failure'); END;",
        )
        .unwrap();
    assert!(matches!(
        execute(&app, &proposal, authorization),
        ControllerMemoryMutationExecutionResult::MutationFailed {
            operation: ControllerMemoryMutationOperation::Correct
        }
    ));
    assert_eq!(grant.remaining_actions().unwrap(), 0);
    assert_eq!(
        app.memories().unwrap().history(&original.id).unwrap().len(),
        1
    );
}

#[test]
fn grant_is_not_persisted_or_reconstructed_after_reopen() {
    let (directory, app, project_id) = open_app("maintenance restart");
    let path = directory.path().join(".orc/orc.db");
    let registry = directory.path().join(".orc/global.db");
    let target = target(&app, project_id, MemoryKind::Episodic, "restart-target");
    let proposal = proposal(&app, &target, ControllerMemoryMutationOperation::Remove);
    let grant = app.create_controller_memory_maintenance_grant(2).unwrap();
    let authorization = app
        .inspect_controller_memory_maintenance_grant(&grant, &proposal)
        .unwrap();
    execute(&app, &proposal, authorization);
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    drop(grant);
    drop(app);

    let reopened = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    assert_eq!(
        reopened
            .memories()
            .unwrap()
            .history(&target.id)
            .unwrap()
            .len(),
        1
    );
    let fresh_grant = reopened
        .create_controller_memory_maintenance_grant(2)
        .unwrap();
    assert_eq!(fresh_grant.remaining_actions().unwrap(), 2);
    assert_eq!(
        fresh_grant.state(),
        ControllerMemoryMaintenanceGrantState::Active
    );
}

#[test]
fn maintenance_grant_creation_does_not_invoke_judgment_or_mutate_memory() {
    let (_directory, app, _project_id) = open_app("maintenance no automation");
    let grant = app.create_controller_memory_maintenance_grant(1).unwrap();
    assert_eq!(grant.remaining_actions().unwrap(), 1);
    assert!(app.memories().unwrap().list(None, true).unwrap().is_empty());
}
