use orc::app::OrcApp;
use orc::memory::{
    MemoryDraft, MemoryId, MemoryKind, MemoryLifecycle, MemoryProvenance, MemoryProvenanceKind,
    MemoryQuery, MemoryScope,
};
use orc::storage::Database;
use tempfile::tempdir;

fn draft(kind: MemoryKind, scope: MemoryScope, subject: &str, content: &str) -> MemoryDraft {
    MemoryDraft {
        kind,
        scope,
        subject: subject.into(),
        content: content.into(),
        provenance: MemoryProvenance {
            kind: MemoryProvenanceKind::Operator,
            source_reference: Some("operator:test".into()),
        },
        confidence: Some(0.9),
    }
}

#[test]
fn memory_kinds_scopes_and_bounded_values_are_deterministic() {
    let directory = tempdir().unwrap();
    let db = Database::init(directory.path().join("orc.db")).unwrap();
    let project = db.create_project("memory").unwrap();

    assert!(
        db.create_memory(&draft(
            MemoryKind::User,
            MemoryScope::Project {
                project_id: project
            },
            "preference",
            "invalid scope",
        ))
        .is_err()
    );
    assert!(
        db.create_memory(&draft(
            MemoryKind::Project,
            MemoryScope::Global,
            "fact",
            "invalid scope",
        ))
        .is_err()
    );
    assert!(
        db.create_memory(&MemoryDraft {
            confidence: Some(1.1),
            ..draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: project
                },
                "fact",
                "invalid confidence",
            )
        })
        .is_err()
    );
    assert!(
        db.create_memory(&MemoryDraft {
            subject: "x".repeat(257),
            ..draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: project
                },
                "fact",
                "oversized subject",
            )
        })
        .is_err()
    );
}

#[test]
fn project_memory_crud_correction_removal_and_history_survive_reopen() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let (project, other_project, original_id, replacement_id) = {
        let db = Database::init(&path).unwrap();
        let project = db.create_project("first").unwrap();
        let other_project = db.create_project("second").unwrap();
        let original = db
            .create_memory(&draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: project,
                },
                "build-command",
                "cargo test --lib",
            ))
            .unwrap();
        let episodic = db
            .create_memory(&draft(
                MemoryKind::Episodic,
                MemoryScope::Project {
                    project_id: project,
                },
                "release",
                "operator released version one",
            ))
            .unwrap();
        assert_eq!(episodic.lifecycle, MemoryLifecycle::Active);
        assert_eq!(
            db.list_memories(&MemoryQuery::active(MemoryScope::Project {
                project_id: other_project,
            }))
            .unwrap(),
            Vec::<orc::memory::MemoryRecord>::new()
        );
        let replacement = db
            .correct_memory(
                &original.id,
                &draft(
                    MemoryKind::Project,
                    MemoryScope::Project {
                        project_id: project,
                    },
                    "build-command",
                    "cargo test --all-targets",
                ),
            )
            .unwrap();
        assert_eq!(
            db.get_memory(&original.id).unwrap().unwrap().lifecycle,
            MemoryLifecycle::Superseded
        );
        assert_eq!(
            db.list_memories(&MemoryQuery {
                scope: MemoryScope::Project {
                    project_id: project
                },
                kind: Some(MemoryKind::Project),
                subject: Some("build-command".into()),
                include_historical: false,
            })
            .unwrap()
            .iter()
            .map(|memory| memory.content.as_str())
            .collect::<Vec<_>>(),
            vec!["cargo test --all-targets"]
        );
        assert_eq!(
            db.memory_history(&original.id)
                .unwrap()
                .iter()
                .map(|memory| memory.id.value())
                .collect::<Vec<_>>(),
            vec![original.id.value(), replacement.id.value()]
        );
        let removed = db.remove_memory(&replacement.id).unwrap();
        assert_eq!(removed.lifecycle, MemoryLifecycle::Removed);
        assert!(
            db.list_memories(&MemoryQuery::active(MemoryScope::Project {
                project_id: project,
            }))
            .unwrap()
            .iter()
            .all(|memory| memory.kind == MemoryKind::Episodic)
        );
        (project, other_project, original.id, replacement.id)
    };
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.get_memory(&MemoryId::Project {
            project_id: project,
            id: original_id.value(),
        })
        .unwrap()
        .unwrap()
        .lifecycle,
        MemoryLifecycle::Superseded
    );
    assert_eq!(
        db.get_memory(&MemoryId::Project {
            project_id: other_project,
            id: replacement_id.value(),
        })
        .unwrap(),
        None
    );
    assert_eq!(db.memory_history(&replacement_id).unwrap().len(), 2);
}

#[test]
fn global_user_and_experience_memory_share_authority_across_projects_and_reset() {
    let directory = tempdir().unwrap();
    let registry = directory.path().join("global.db");
    let first_path = directory.path().join("first/orc.db");
    let second_path = directory.path().join("second/orc.db");
    let user_id = {
        let db = Database::init_with_registry(&first_path, &registry).unwrap();
        let project = db.create_project("first").unwrap();
        let project_memory = db
            .create_memory(&draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: project,
                },
                "private",
                "only in first project",
            ))
            .unwrap();
        let user = db
            .create_memory(&draft(
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
            "run migrations before reopening a legacy database",
        ))
        .unwrap();
        assert!(matches!(user.id, MemoryId::Global(_)));
        assert!(matches!(project_memory.id, MemoryId::Project { .. }));
        user.id.value()
    };

    let second = Database::init_with_registry(&second_path, &registry).unwrap();
    let second_project = second.create_project("second").unwrap();
    let global = second
        .list_memories(&MemoryQuery::active(MemoryScope::Global))
        .unwrap();
    assert_eq!(global.len(), 2);
    assert!(
        global
            .iter()
            .any(|memory| memory.id == MemoryId::Global(user_id))
    );
    assert!(
        second
            .list_memories(&MemoryQuery::active(MemoryScope::Project {
                project_id: second_project,
            }))
            .unwrap()
            .is_empty()
    );
    drop(second);

    std::fs::remove_file(&first_path).unwrap();
    let replacement = Database::init_with_registry(&first_path, &registry).unwrap();
    let replacement_project = replacement.create_project("replacement").unwrap();
    assert_eq!(
        replacement
            .list_memories(&MemoryQuery::active(MemoryScope::Global))
            .unwrap()
            .len(),
        2
    );
    assert!(
        replacement
            .list_memories(&MemoryQuery::active(MemoryScope::Project {
                project_id: replacement_project,
            }))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn global_correction_and_removal_are_historical_and_transactional() {
    let directory = tempdir().unwrap();
    let db = Database::init(directory.path().join("orc.db")).unwrap();
    let original = db
        .create_memory(&draft(
            MemoryKind::Experience,
            MemoryScope::Global,
            "review",
            "inspect the current diff",
        ))
        .unwrap();
    let replacement = db
        .supersede_memory(
            &original.id,
            &draft(
                MemoryKind::Experience,
                MemoryScope::Global,
                "review",
                "inspect the current diff and validation evidence",
            ),
        )
        .unwrap();
    assert_eq!(db.memory_history(&original.id).unwrap().len(), 2);
    assert_eq!(
        db.remove_memory(&original.id).unwrap_err().to_string(),
        "scheduler error: only an active memory can be removed"
    );
    assert_eq!(
        db.remove_memory(&replacement.id).unwrap().lifecycle,
        MemoryLifecycle::Removed
    );
    assert!(
        db.list_memories(&MemoryQuery::active(MemoryScope::Global))
            .unwrap()
            .is_empty()
    );
    assert_eq!(db.memory_history(&original.id).unwrap().len(), 2);
}

#[test]
fn application_memory_facade_enforces_current_project_scope() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("orc.db");
    let registry = directory.path().join("global.db");
    let (first, second, second_memory) = {
        let db = Database::init_with_registry(&path, &registry).unwrap();
        let first = db.create_project("first").unwrap();
        let second = db.create_project("second").unwrap();
        let second_memory = db
            .create_memory(&draft(
                MemoryKind::Project,
                MemoryScope::Project { project_id: second },
                "private",
                "second only",
            ))
            .unwrap();
        (first, second, second_memory)
    };
    assert_ne!(first, second);
    let app = OrcApp::open_with_registry(&path, directory.path(), &registry).unwrap();
    let memories = app.memories().unwrap();
    assert!(memories.get(&second_memory.id).unwrap().is_none());
    assert!(memories.remove(&second_memory.id).is_err());
}
