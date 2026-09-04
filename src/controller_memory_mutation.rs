//! Supervised, typed Controller proposals for canonical memory mutation.
//!
//! The Controller can describe one bounded memory mutation, but it receives no
//! storage or mutation capability. Orc mints a one-shot authorization only for
//! a proposal that passes deterministic canonical checks; execution validates
//! fresh state and delegates to the existing MemoryService operations.

use crate::memory::{
    MemoryDraft, MemoryId, MemoryLifecycle, MemoryRecord, MemoryScope, MemoryService,
};
use crate::storage::DbError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_CONTROLLER_MEMORY_MUTATION_INTENT_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerMemoryMutationOperation {
    Create,
    Correct,
    Supersede,
    Remove,
}

/// Bounded declarative memory mutations. This type contains no storage,
/// persistence, registry, filesystem, workflow, or execution capability.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerMemoryMutationIntent {
    Create {
        draft: MemoryDraft,
    },
    Correct {
        target: MemoryId,
        replacement: MemoryDraft,
    },
    Supersede {
        target: MemoryId,
        replacement: MemoryDraft,
    },
    Remove {
        target: MemoryId,
    },
}

impl ControllerMemoryMutationIntent {
    pub const fn operation(&self) -> ControllerMemoryMutationOperation {
        match self {
            Self::Create { .. } => ControllerMemoryMutationOperation::Create,
            Self::Correct { .. } => ControllerMemoryMutationOperation::Correct,
            Self::Supersede { .. } => ControllerMemoryMutationOperation::Supersede,
            Self::Remove { .. } => ControllerMemoryMutationOperation::Remove,
        }
    }

    pub fn validate(&self) -> Result<(), ControllerMemoryMutationError> {
        match self {
            Self::Create { draft } => draft
                .validate()
                .map_err(|error| ControllerMemoryMutationError::InvalidIntent(error.to_string()))?,
            Self::Correct {
                target,
                replacement,
            }
            | Self::Supersede {
                target,
                replacement,
            } => {
                validate_target(target)?;
                replacement.validate().map_err(|error| {
                    ControllerMemoryMutationError::InvalidIntent(error.to_string())
                })?;
                if replacement.scope != target.scope() {
                    return Err(ControllerMemoryMutationError::ScopeMismatch);
                }
            }
            Self::Remove { target } => validate_target(target)?,
        }
        let size = serde_json::to_vec(self)
            .map_err(|error| ControllerMemoryMutationError::InvalidIntent(error.to_string()))?
            .len();
        if size > MAX_CONTROLLER_MEMORY_MUTATION_INTENT_BYTES {
            return Err(ControllerMemoryMutationError::IntentTooLarge { actual: size });
        }
        Ok(())
    }

    fn target(&self) -> Option<&MemoryId> {
        match self {
            Self::Create { .. } => None,
            Self::Correct { target, .. }
            | Self::Supersede { target, .. }
            | Self::Remove { target } => Some(target),
        }
    }

    fn replacement(&self) -> Option<&MemoryDraft> {
        match self {
            Self::Correct { replacement, .. } | Self::Supersede { replacement, .. } => {
                Some(replacement)
            }
            Self::Create { .. } | Self::Remove { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerMemoryMutationRejection {
    InvalidIntent,
    InvalidProjectBinding,
    InvalidTarget,
    TargetNotFound,
    TargetNotActive,
    TargetKindMismatch,
    TargetSubjectMismatch,
    ScopeMismatch,
    StorageReadFailed,
}

#[derive(Debug, Error)]
pub enum ControllerMemoryMutationError {
    #[error("Controller memory mutation has no active project")]
    NoActiveProject,
    #[error("Controller memory mutation intent is invalid: {0}")]
    InvalidIntent(String),
    #[error(
        "Controller memory mutation intent is {actual} bytes; maximum is {MAX_CONTROLLER_MEMORY_MUTATION_INTENT_BYTES}"
    )]
    IntentTooLarge { actual: usize },
    #[error("Controller memory mutation is outside the current project")]
    InvalidProjectBinding,
    #[error("Controller memory mutation target is invalid")]
    InvalidTarget,
    #[error("Controller memory mutation target was not found")]
    TargetNotFound,
    #[error("Controller memory mutation target is not active")]
    TargetNotActive,
    #[error("Controller memory mutation target kind does not match replacement")]
    TargetKindMismatch,
    #[error("Controller memory mutation target subject does not match replacement")]
    TargetSubjectMismatch,
    #[error("Controller memory mutation scope does not match its target")]
    ScopeMismatch,
    #[error("Controller memory mutation storage read failed: {0}")]
    Storage(#[source] DbError),
    #[error("Controller memory mutation memory service read failed: {0}")]
    MemoryService(String),
}

/// A canonical legality result used by the proposal and execution boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ControllerMemoryMutationLegality {
    Allowed,
    Rejected {
        reason: ControllerMemoryMutationRejection,
    },
}

/// A validated proposal. Its fields are private so callers cannot manufacture
/// a proposal with a project binding that was not checked by Orc.
#[derive(Clone, Debug, PartialEq)]
pub struct ControllerMemoryMutationProposal {
    project_id: i64,
    intent: ControllerMemoryMutationIntent,
}

impl ControllerMemoryMutationProposal {
    pub fn intent(&self) -> &ControllerMemoryMutationIntent {
        &self.intent
    }

    pub const fn project_id(&self) -> i64 {
        self.project_id
    }

    pub fn operation(&self) -> ControllerMemoryMutationOperation {
        self.intent.operation()
    }

    pub(crate) fn from_intent(
        project_id: i64,
        intent: ControllerMemoryMutationIntent,
        memories: &MemoryService<'_>,
    ) -> Result<Self, ControllerMemoryMutationError> {
        validate_for_current_project(project_id, &intent, memories)?;
        Ok(Self { project_id, intent })
    }

    fn validate_fresh(
        &self,
        project_id: i64,
        memories: &MemoryService<'_>,
    ) -> Result<(), ControllerMemoryMutationRejection> {
        validate_for_current_project(project_id, &self.intent, memories)
            .map_err(rejection_for_error)
    }
}

/// Opaque, non-serializable authorization for one exact proposal. Passing it
/// by value to execution makes it one-shot; it cannot be cloned or replayed.
#[derive(Debug)]
pub struct ControllerMemoryMutationAuthorization {
    project_id: i64,
    intent: ControllerMemoryMutationIntent,
}

impl ControllerMemoryMutationAuthorization {
    fn matches(&self, proposal: &ControllerMemoryMutationProposal) -> bool {
        self.project_id == proposal.project_id && self.intent == proposal.intent
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerMemoryMutationAuthorizationRejection {
    Missing,
    NotAuthorizedForIntent,
}

/// Canonical typed result of one supervised memory mutation attempt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ControllerMemoryMutationExecutionResult {
    AuthorizationRejected {
        operation: ControllerMemoryMutationOperation,
        reason: ControllerMemoryMutationAuthorizationRejection,
    },
    FreshValidationRejected {
        operation: ControllerMemoryMutationOperation,
        reason: ControllerMemoryMutationRejection,
    },
    Mutated {
        operation: ControllerMemoryMutationOperation,
        record: Box<MemoryRecord>,
    },
    MutationFailed {
        operation: ControllerMemoryMutationOperation,
    },
}

fn validate_target(target: &MemoryId) -> Result<(), ControllerMemoryMutationError> {
    if target.value() <= 0 {
        return Err(ControllerMemoryMutationError::InvalidTarget);
    }
    Ok(())
}

fn validate_for_current_project(
    project_id: i64,
    intent: &ControllerMemoryMutationIntent,
    memories: &MemoryService<'_>,
) -> Result<(), ControllerMemoryMutationError> {
    if project_id <= 0 {
        return Err(ControllerMemoryMutationError::InvalidProjectBinding);
    }
    intent.validate()?;
    if let Some(target) = intent.target() {
        validate_project_binding(project_id, &target.scope())?;
        let current = memories
            .get(target)
            .map_err(ControllerMemoryMutationError::Storage)?
            .ok_or(ControllerMemoryMutationError::TargetNotFound)?;
        if current.lifecycle != MemoryLifecycle::Active {
            return Err(ControllerMemoryMutationError::TargetNotActive);
        }
        if let Some(replacement) = intent.replacement() {
            if current.kind != replacement.kind {
                return Err(ControllerMemoryMutationError::TargetKindMismatch);
            }
            if current.subject != replacement.subject {
                return Err(ControllerMemoryMutationError::TargetSubjectMismatch);
            }
        }
    }
    if let ControllerMemoryMutationIntent::Create { draft } = intent {
        validate_project_binding(project_id, &draft.scope)?;
    }
    Ok(())
}

fn validate_project_binding(
    project_id: i64,
    scope: &MemoryScope,
) -> Result<(), ControllerMemoryMutationError> {
    match scope {
        MemoryScope::Global => Ok(()),
        MemoryScope::Project { project_id: owner } if *owner == project_id => Ok(()),
        MemoryScope::Project { .. } => Err(ControllerMemoryMutationError::InvalidProjectBinding),
    }
}

fn rejection_for_error(error: ControllerMemoryMutationError) -> ControllerMemoryMutationRejection {
    match error {
        ControllerMemoryMutationError::InvalidIntent(_)
        | ControllerMemoryMutationError::IntentTooLarge { .. } => {
            ControllerMemoryMutationRejection::InvalidIntent
        }
        ControllerMemoryMutationError::InvalidProjectBinding => {
            ControllerMemoryMutationRejection::InvalidProjectBinding
        }
        ControllerMemoryMutationError::InvalidTarget => {
            ControllerMemoryMutationRejection::InvalidTarget
        }
        ControllerMemoryMutationError::TargetNotFound => {
            ControllerMemoryMutationRejection::TargetNotFound
        }
        ControllerMemoryMutationError::TargetNotActive => {
            ControllerMemoryMutationRejection::TargetNotActive
        }
        ControllerMemoryMutationError::TargetKindMismatch => {
            ControllerMemoryMutationRejection::TargetKindMismatch
        }
        ControllerMemoryMutationError::TargetSubjectMismatch => {
            ControllerMemoryMutationRejection::TargetSubjectMismatch
        }
        ControllerMemoryMutationError::ScopeMismatch => {
            ControllerMemoryMutationRejection::ScopeMismatch
        }
        ControllerMemoryMutationError::Storage(_)
        | ControllerMemoryMutationError::NoActiveProject => {
            ControllerMemoryMutationRejection::StorageReadFailed
        }
        ControllerMemoryMutationError::MemoryService(_) => {
            ControllerMemoryMutationRejection::StorageReadFailed
        }
    }
}

pub(crate) fn authorize(
    proposal: &ControllerMemoryMutationProposal,
) -> ControllerMemoryMutationAuthorization {
    ControllerMemoryMutationAuthorization {
        project_id: proposal.project_id,
        intent: proposal.intent.clone(),
    }
}

pub(crate) fn execute(
    proposal: &ControllerMemoryMutationProposal,
    authorization: Option<ControllerMemoryMutationAuthorization>,
    project_id: i64,
    memories: &MemoryService<'_>,
) -> ControllerMemoryMutationExecutionResult {
    let operation = proposal.operation();
    let Some(authorization) = authorization else {
        return ControllerMemoryMutationExecutionResult::AuthorizationRejected {
            operation,
            reason: ControllerMemoryMutationAuthorizationRejection::Missing,
        };
    };
    if !authorization.matches(proposal) {
        return ControllerMemoryMutationExecutionResult::AuthorizationRejected {
            operation,
            reason: ControllerMemoryMutationAuthorizationRejection::NotAuthorizedForIntent,
        };
    }
    if let Err(reason) = proposal.validate_fresh(project_id, memories) {
        return ControllerMemoryMutationExecutionResult::FreshValidationRejected {
            operation,
            reason,
        };
    }
    let mutation = match &proposal.intent {
        ControllerMemoryMutationIntent::Create { draft } => memories.create(draft),
        ControllerMemoryMutationIntent::Correct {
            target,
            replacement,
        } => memories.correct(target, replacement),
        ControllerMemoryMutationIntent::Supersede {
            target,
            replacement,
        } => memories.supersede(target, replacement),
        ControllerMemoryMutationIntent::Remove { target } => memories.remove(target),
    };
    match mutation {
        Ok(record) => ControllerMemoryMutationExecutionResult::Mutated {
            operation,
            record: Box::new(record),
        },
        Err(_) => ControllerMemoryMutationExecutionResult::MutationFailed { operation },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryKind, MemoryProvenance, MemoryProvenanceKind, MemoryScope};
    use crate::storage::Database;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Database, i64) {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let project_id = db.create_project("memory mutation").unwrap();
        (directory, db, project_id)
    }

    fn draft(
        kind: MemoryKind,
        scope: crate::memory::MemoryScope,
        subject: &str,
        content: &str,
    ) -> MemoryDraft {
        MemoryDraft {
            kind,
            scope,
            subject: subject.into(),
            content: content.into(),
            provenance: MemoryProvenance {
                kind: MemoryProvenanceKind::ControllerApproved,
                source_reference: Some("controller:memory-mutation-test".into()),
            },
            confidence: Some(0.8),
        }
    }

    #[test]
    fn each_intent_serializes_and_validates_with_typed_data() {
        let (_directory, db, project_id) = setup();
        let service = MemoryService::new(&db, project_id);
        let original = service
            .create(&draft(
                MemoryKind::Project,
                MemoryScope::Project { project_id },
                "fact",
                "one",
            ))
            .unwrap();
        let intents = vec![
            ControllerMemoryMutationIntent::Create {
                draft: draft(MemoryKind::User, MemoryScope::Global, "preference", "two"),
            },
            ControllerMemoryMutationIntent::Correct {
                target: original.id.clone(),
                replacement: draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "fact",
                    "corrected",
                ),
            },
            ControllerMemoryMutationIntent::Supersede {
                target: original.id.clone(),
                replacement: draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "fact",
                    "superseded",
                ),
            },
            ControllerMemoryMutationIntent::Remove {
                target: original.id,
            },
        ];
        for intent in intents {
            intent.validate().unwrap();
            let encoded = serde_json::to_value(&intent).unwrap();
            assert!(encoded.get("operation").is_some());
        }
    }

    #[test]
    fn project_global_scope_matrix_and_cross_project_targets_are_rejected() {
        let (_directory, db, project_id) = setup();
        let other_project = db.create_project("other").unwrap();
        let service = MemoryService::new(&db, project_id);
        assert!(matches!(
            ControllerMemoryMutationIntent::Create {
                draft: draft(
                    MemoryKind::User,
                    MemoryScope::Project { project_id },
                    "invalid",
                    "value",
                ),
            }
            .validate(),
            Err(ControllerMemoryMutationError::InvalidIntent(_))
        ));
        assert!(matches!(
            ControllerMemoryMutationIntent::Create {
                draft: draft(MemoryKind::Project, MemoryScope::Global, "invalid", "value"),
            }
            .validate(),
            Err(ControllerMemoryMutationError::InvalidIntent(_))
        ));
        let project_draft = draft(
            MemoryKind::Project,
            MemoryScope::Project { project_id },
            "project",
            "value",
        );
        let global_draft = draft(
            MemoryKind::Experience,
            MemoryScope::Global,
            "experience",
            "value",
        );
        ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Create {
                draft: project_draft,
            },
            &service,
        )
        .unwrap();
        ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Create {
                draft: global_draft,
            },
            &service,
        )
        .unwrap();
        let outside = draft(
            MemoryKind::Project,
            MemoryScope::Project {
                project_id: other_project,
            },
            "outside",
            "value",
        );
        assert!(matches!(
            ControllerMemoryMutationProposal::from_intent(
                project_id,
                ControllerMemoryMutationIntent::Create { draft: outside },
                &service,
            ),
            Err(ControllerMemoryMutationError::InvalidProjectBinding)
        ));
        let other_memory = db
            .create_memory(&draft(
                MemoryKind::Project,
                MemoryScope::Project {
                    project_id: other_project,
                },
                "other",
                "value",
            ))
            .unwrap();
        assert!(matches!(
            ControllerMemoryMutationProposal::from_intent(
                project_id,
                ControllerMemoryMutationIntent::Remove {
                    target: other_memory.id,
                },
                &service,
            ),
            Err(ControllerMemoryMutationError::InvalidProjectBinding)
        ));
    }

    #[test]
    fn authorization_is_opaque_exact_and_one_shot() {
        let (_directory, db, project_id) = setup();
        let service = MemoryService::new(&db, project_id);
        let intent = ControllerMemoryMutationIntent::Create {
            draft: draft(MemoryKind::User, MemoryScope::Global, "preference", "value"),
        };
        let proposal =
            ControllerMemoryMutationProposal::from_intent(project_id, intent, &service).unwrap();
        let mismatched = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Create {
                draft: draft(MemoryKind::User, MemoryScope::Global, "other", "value"),
            },
            &service,
        )
        .unwrap();
        let before = service.list(None, true).unwrap();
        let authorization = authorize(&proposal);
        assert!(matches!(
            execute(&mismatched, Some(authorization), project_id, &service),
            ControllerMemoryMutationExecutionResult::AuthorizationRejected {
                reason: ControllerMemoryMutationAuthorizationRejection::NotAuthorizedForIntent,
                ..
            }
        ));
        assert_eq!(service.list(None, true).unwrap(), before);
        let authorization = authorize(&proposal);
        assert!(matches!(
            execute(&proposal, Some(authorization), project_id, &service),
            ControllerMemoryMutationExecutionResult::Mutated { .. }
        ));
        assert!(matches!(
            execute(&proposal, None, project_id, &service),
            ControllerMemoryMutationExecutionResult::AuthorizationRejected {
                reason: ControllerMemoryMutationAuthorizationRejection::Missing,
                ..
            }
        ));
    }

    #[test]
    fn target_lifecycle_and_stale_state_are_revalidated_without_mutation() {
        let (_directory, db, project_id) = setup();
        let service = MemoryService::new(&db, project_id);
        let original = service
            .create(&draft(
                MemoryKind::Project,
                MemoryScope::Project { project_id },
                "fact",
                "one",
            ))
            .unwrap();
        let proposal = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Remove {
                target: original.id.clone(),
            },
            &service,
        )
        .unwrap();
        service.remove(&original.id).unwrap();
        let before = service.history(&original.id).unwrap();
        let result = execute(&proposal, Some(authorize(&proposal)), project_id, &service);
        assert!(matches!(
            result,
            ControllerMemoryMutationExecutionResult::FreshValidationRejected {
                reason: ControllerMemoryMutationRejection::TargetNotActive,
                ..
            }
        ));
        assert_eq!(service.history(&original.id).unwrap(), before);
    }

    #[test]
    fn canonical_create_correct_supersede_remove_paths_preserve_history() {
        let (_directory, db, project_id) = setup();
        let service = MemoryService::new(&db, project_id);
        let create = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Create {
                draft: draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "fact",
                    "one",
                ),
            },
            &service,
        )
        .unwrap();
        let created = match execute(&create, Some(authorize(&create)), project_id, &service) {
            ControllerMemoryMutationExecutionResult::Mutated { record, .. } => *record,
            other => panic!("unexpected result: {other:?}"),
        };
        let correct = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Correct {
                target: created.id.clone(),
                replacement: draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "fact",
                    "two",
                ),
            },
            &service,
        )
        .unwrap();
        let corrected = match execute(&correct, Some(authorize(&correct)), project_id, &service) {
            ControllerMemoryMutationExecutionResult::Mutated { record, .. } => *record,
            other => panic!("unexpected result: {other:?}"),
        };
        assert_eq!(corrected.supersedes, Some(created.id.clone()));
        let supersede = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Supersede {
                target: corrected.id.clone(),
                replacement: draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "fact",
                    "three",
                ),
            },
            &service,
        )
        .unwrap();
        let superseded = match execute(
            &supersede,
            Some(authorize(&supersede)),
            project_id,
            &service,
        ) {
            ControllerMemoryMutationExecutionResult::Mutated { record, .. } => *record,
            other => panic!("unexpected result: {other:?}"),
        };
        assert_eq!(superseded.supersedes, Some(corrected.id.clone()));
        let remove = ControllerMemoryMutationProposal::from_intent(
            project_id,
            ControllerMemoryMutationIntent::Remove {
                target: superseded.id.clone(),
            },
            &service,
        )
        .unwrap();
        let removed = match execute(&remove, Some(authorize(&remove)), project_id, &service) {
            ControllerMemoryMutationExecutionResult::Mutated { record, .. } => *record,
            other => panic!("unexpected result: {other:?}"),
        };
        assert_eq!(removed.lifecycle, MemoryLifecycle::Removed);
        assert_eq!(service.history(&created.id).unwrap().len(), 3);
    }

    #[test]
    fn global_memory_is_registry_owned_while_project_memory_is_bound() {
        let (directory, db, project_id) = setup();
        let path = directory.path().join("orc.db");
        let (global, project) = {
            let service = MemoryService::new(&db, project_id);
            let global = service
                .create(&draft(
                    MemoryKind::User,
                    MemoryScope::Global,
                    "user",
                    "keep",
                ))
                .unwrap();
            let project = service
                .create(&draft(
                    MemoryKind::Project,
                    MemoryScope::Project { project_id },
                    "project",
                    "bound",
                ))
                .unwrap();
            (global, project)
        };
        drop(db);
        std::fs::remove_file(&path).unwrap();
        let reopened = Database::init(&path).unwrap();
        assert!(reopened.get_memory(&global.id).unwrap().is_some());
        assert!(reopened.get_memory(&project.id).unwrap().is_none());
    }
}
