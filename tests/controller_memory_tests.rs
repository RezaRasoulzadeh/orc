use orc::app::OrcApp;
use orc::controller::ControllerStatePacket;
use orc::controller_intake::ControllerIntakeRequest;
use orc::controller_memory::{
    ControllerMemoryAuthority, ControllerMemoryContext, ControllerMemoryItem,
    MAX_CONTROLLER_MEMORY_CONTENT_BYTES, MAX_CONTROLLER_MEMORY_CONTEXT_BYTES,
    MAX_CONTROLLER_MEMORY_ITEMS, MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND,
};
use orc::controller_plan_review::ControllerPlanReviewRequest;
use orc::controller_planning::ControllerPlanningRequest;
use orc::memory::{
    MemoryDraft, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind, MemoryScope,
};
use orc::storage::Database;
use std::marker::PhantomData;
use tempfile::tempdir;

fn draft(kind: MemoryKind, scope: MemoryScope, subject: &str, content: &str) -> MemoryDraft {
    MemoryDraft {
        kind,
        scope,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("operator:controller-memory-test".into()),
        },
        confidence: Some(0.75),
    }
}

fn open_app(path: &std::path::Path, root: &std::path::Path, registry: &std::path::Path) -> OrcApp {
    OrcApp::open_with_registry(path, root, registry).unwrap()
}

#[test]
fn empty_context_is_valid_and_available_through_orc_app() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    db.create_project("empty").unwrap();
    drop(db);

    let app = open_app(&path, directory.path(), &registry);
    let context = app.controller_memory_context().unwrap();
    assert_eq!(context, ControllerMemoryContext::empty());
    context.validate().unwrap();
}

#[test]
fn context_preserves_authority_metadata_and_exact_project_isolation() {
    let directory = tempdir().unwrap();
    let registry = directory.path().join("global.db");
    let first_path = directory.path().join("first/orc.db");
    let second_path = directory.path().join("second/orc.db");
    let (first_project, first_project_memory_id) = {
        let db = Database::init_with_registry(&first_path, &registry).unwrap();
        let first_project = db.create_project("first").unwrap();
        let second_project = db.create_project("second").unwrap();
        let first_memory = db
            .create_memory(&draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: first_project,
                },
                "build-command",
                "cargo test --lib",
            ))
            .unwrap();
        db.create_memory(&draft(
            MemoryKind::Episodic,
            MemoryScope::Project {
                project_id: first_project,
            },
            "release",
            "operator released version one",
        ))
        .unwrap();
        db.create_memory(&draft(
            MemoryKind::Project,
            MemoryScope::Project {
                project_id: second_project,
            },
            "private",
            "second project only",
        ))
        .unwrap();
        db.create_memory(&draft(
            MemoryKind::User,
            MemoryScope::Global,
            "editor",
            "prefer concise explanations",
        ))
        .unwrap();
        db.create_memory(&draft(
            MemoryKind::Experience,
            MemoryScope::Global,
            "migration",
            "run migrations before reopening",
        ))
        .unwrap();
        (first_project, first_memory.id)
    };

    let first = open_app(&first_path, directory.path(), &registry);
    let first_context = first.controller_memory_context().unwrap();
    assert_eq!(first_context.items.len(), 4);
    assert!(
        first_context
            .items
            .iter()
            .all(|item| item.content != "second project only")
    );
    let project_item = first_context
        .items
        .iter()
        .find(|item| item.id == first_project_memory_id)
        .unwrap();
    assert_eq!(project_item.kind, MemoryKind::Project);
    assert_eq!(
        project_item.scope,
        MemoryScope::Project {
            project_id: first_project
        }
    );
    assert_eq!(
        project_item.authority,
        ControllerMemoryAuthority::CurrentProject
    );
    assert_eq!(project_item.provenance.kind, MemoryProvenanceKind::Operator);
    assert_eq!(project_item.confidence, Some(0.75));
    assert_eq!(project_item.lifecycle, MemoryLifecycle::Active);

    let second_db = Database::init_with_registry(&second_path, &registry).unwrap();
    second_db.create_project("second database").unwrap();
    drop(second_db);
    let second = open_app(&second_path, directory.path(), &registry);
    let second_context = second.controller_memory_context().unwrap();
    assert!(
        second_context
            .items
            .iter()
            .any(|item| item.authority == ControllerMemoryAuthority::DurableUser)
    );
    assert!(
        second_context
            .items
            .iter()
            .any(|item| item.authority == ControllerMemoryAuthority::CrossProjectExperience)
    );
    assert!(
        second_context
            .items
            .iter()
            .all(|item| item.content != "cargo test --lib"
                && item.content != "operator released version one")
    );
}

#[test]
fn removed_and_superseded_records_are_excluded_without_read_mutation() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project = db.create_project("history").unwrap();
    let original = db
        .create_memory(&draft(
            MemoryKind::Project,
            MemoryScope::Project {
                project_id: project,
            },
            "command",
            "cargo test",
        ))
        .unwrap();
    let replacement = db
        .correct_memory(
            &original.id,
            &draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: project,
                },
                "command",
                "cargo test --all-targets",
            ),
        )
        .unwrap();
    db.remove_memory(&replacement.id).unwrap();
    let before_records = db
        .list_memories(&orc::memory::MemoryQuery {
            scope: MemoryScope::Project {
                project_id: project,
            },
            kind: Some(MemoryKind::Project),
            subject: Some("command".into()),
            include_historical: true,
        })
        .unwrap();
    let before_history = db.memory_history(&original.id).unwrap();
    drop(db);

    let app = open_app(&path, directory.path(), &registry);
    let context = app.controller_memory_context().unwrap();
    assert_eq!(context, app.controller_memory_context().unwrap());
    assert!(context.items.is_empty());
    let memories = app.memories().unwrap();
    let after_records = memories.list(Some(MemoryKind::Project), true).unwrap();
    let after_history = memories.history(&original.id).unwrap();
    assert_eq!(after_records, before_records);
    assert_eq!(after_history, before_history);
    assert_eq!(after_history.len(), 2);
    assert_eq!(after_history[0].lifecycle, MemoryLifecycle::Superseded);
    assert_eq!(after_history[1].lifecycle, MemoryLifecycle::Removed);
}

#[test]
fn ordering_and_count_bounds_are_stable_and_serialized_context_is_bounded() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project = db.create_project("bounds").unwrap();
    for index in 0..10 {
        for kind in [
            MemoryKind::Project,
            MemoryKind::User,
            MemoryKind::Episodic,
            MemoryKind::Experience,
        ] {
            let scope = if kind.is_global() {
                MemoryScope::Global
            } else {
                MemoryScope::Project {
                    project_id: project,
                }
            };
            db.create_memory(&draft(
                kind,
                scope,
                &format!("subject-{index}"),
                &format!("content-{index}"),
            ))
            .unwrap();
        }
    }
    drop(db);

    let app = open_app(&path, directory.path(), &registry);
    let first = app.controller_memory_context().unwrap();
    let second = app.controller_memory_context().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.items.len(), MAX_CONTROLLER_MEMORY_ITEMS);
    for kind in [
        MemoryKind::Project,
        MemoryKind::User,
        MemoryKind::Episodic,
        MemoryKind::Experience,
    ] {
        assert!(
            first.items.iter().filter(|item| item.kind == kind).count()
                <= MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND
        );
    }
    assert_eq!(
        first
            .items
            .iter()
            .filter(|item| item.kind == MemoryKind::Project)
            .count(),
        MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND
    );
    assert_eq!(
        first
            .items
            .iter()
            .filter(|item| item.kind == MemoryKind::User)
            .count(),
        MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND
    );
    assert_eq!(
        first
            .items
            .iter()
            .filter(|item| item.kind == MemoryKind::Episodic)
            .count(),
        MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND
    );
    assert_eq!(
        first
            .items
            .iter()
            .filter(|item| item.kind == MemoryKind::Experience)
            .count(),
        0
    );
    let serialized = serde_json::to_vec(&first).unwrap();
    assert!(serialized.len() <= MAX_CONTROLLER_MEMORY_CONTEXT_BYTES);
    first.validate().unwrap();
}

#[test]
fn serialized_context_bound_truncates_large_active_items_deterministically() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project = db.create_project("serialized-bound").unwrap();
    for index in 0..MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND {
        db.create_memory(&draft(
            MemoryKind::Project,
            MemoryScope::Project {
                project_id: project,
            },
            &format!("large-{index}"),
            &"x".repeat(MAX_CONTROLLER_MEMORY_CONTENT_BYTES),
        ))
        .unwrap();
    }
    drop(db);

    let app = open_app(&path, directory.path(), &registry);
    let first = app.controller_memory_context().unwrap();
    let second = app.controller_memory_context().unwrap();
    assert_eq!(first, second);
    assert!(!first.items.is_empty());
    assert!(first.items.len() < MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND);
    assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_CONTROLLER_MEMORY_CONTEXT_BYTES);
    first.validate().unwrap();
}

#[test]
fn oversized_projection_items_are_omitted_without_utf8_truncation() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project = db.create_project("oversized").unwrap();
    db.create_memory(&draft(
        MemoryKind::Project,
        MemoryScope::Project {
            project_id: project,
        },
        "too-large",
        &"x".repeat(MAX_CONTROLLER_MEMORY_CONTENT_BYTES + 1),
    ))
    .unwrap();
    db.create_memory(&draft(
        MemoryKind::Project,
        MemoryScope::Project {
            project_id: project,
        },
        "kept",
        "bounded content",
    ))
    .unwrap();
    drop(db);

    let app = open_app(&path, directory.path(), &registry);
    let context = app.controller_memory_context().unwrap();
    assert!(context.items.iter().all(|item| item.subject != "too-large"));
    assert!(context.items.iter().any(|item| item.subject == "kept"));
    for item in &context.items {
        item.validate().unwrap();
    }
}

#[test]
fn unprojectable_records_do_not_consume_the_per_kind_quota() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let db = Database::init_with_registry(&path, &registry).unwrap();
    let project = db.create_project("projection-quota").unwrap();
    for index in 0..MAX_CONTROLLER_MEMORY_ITEMS_PER_KIND {
        db.create_memory(&draft(
            MemoryKind::Project,
            MemoryScope::Project {
                project_id: project,
            },
            &format!("oversized-{index}"),
            &"x".repeat(MAX_CONTROLLER_MEMORY_CONTENT_BYTES + 1),
        ))
        .unwrap();
    }
    db.create_memory(&draft(
        MemoryKind::Project,
        MemoryScope::Project {
            project_id: project,
        },
        "bounded-after-oversized",
        "this eligible record must consume the first project-memory slot",
    ))
    .unwrap();
    drop(db);

    let app = open_app(&path, directory.path(), &registry);
    let context = app.controller_memory_context().unwrap();
    assert_eq!(context.items.len(), 1);
    assert_eq!(context.items[0].subject, "bounded-after-oversized");
    assert_eq!(context.items[0].kind, MemoryKind::Project);
}

#[test]
fn projected_item_type_is_standalone_from_existing_controller_packets() {
    fn accepts_memory_context(_context: &ControllerMemoryContext) {}
    fn accepts_memory_item(_item: &ControllerMemoryItem) {}
    fn capability_specific_slot<T>() -> PhantomData<(T, ControllerMemoryContext)> {
        PhantomData
    }

    let context = ControllerMemoryContext::empty();
    accepts_memory_context(&context);
    let _ = accepts_memory_item;
    let _: PhantomData<(ControllerStatePacket, ControllerMemoryContext)> =
        capability_specific_slot();
    let _: PhantomData<(ControllerPlanningRequest, ControllerMemoryContext)> =
        capability_specific_slot();
    let _: PhantomData<(ControllerPlanReviewRequest, ControllerMemoryContext)> =
        capability_specific_slot();
    let _: PhantomData<(ControllerIntakeRequest, ControllerMemoryContext)> =
        capability_specific_slot();
    context.validate().unwrap();
}
