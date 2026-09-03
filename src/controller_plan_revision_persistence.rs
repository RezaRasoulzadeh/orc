//! Trusted, atomic persistence of one validated Controller Plan revision.
//!
//! M05-006 produces a read-only revision result. This module binds that result
//! to the canonical parent and source review, then requires trusted one-shot
//! authorization before the database's atomic revision transaction.

use crate::controller_plan_revision::{ControllerPlanRevisionResult, persisted_revision_feedback};
use crate::protocol::PlanResponse;
use crate::storage::db::{PersistedPlan, PlanOrigin, PlanReview, PlanReviewDecision, PlanStatus};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_REVISION_PERSISTENCE_BYTES: usize = 64 * 1024;
const MAX_REVIEW_DETAILS_BYTES: usize = 8 * 1024;

#[derive(Debug, Error)]
pub enum ControllerPlanRevisionPersistenceProposalError {
    #[error("Controller Plan revision project identity is invalid")]
    InvalidProject,
    #[error("Controller Plan revision result is invalid")]
    InvalidRevisionResult,
    #[error("Controller Plan revision parent is invalid")]
    InvalidParent,
    #[error("Controller Plan revision source review is invalid")]
    InvalidSourceReview,
    #[error("Controller Plan revision review details exceed the persistence bound")]
    ReviewDetailsTooLarge,
    #[error("Controller Plan revision exceeds the persistence bound")]
    RevisionTooLarge,
}

/// A typed proposal bound to the complete canonical parent, source review,
/// and revised Plan. The fields are private so callers cannot deserialize or
/// manufacture durable lineage metadata into the proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerPlanRevisionPersistenceProposal {
    project_id: i64,
    parent_plan_id: i64,
    parent_plan_version: i64,
    parent_plan: PlanResponse,
    source_review_id: i64,
    source_review_details: String,
    revision_feedback: String,
    revised_plan: PlanResponse,
}

impl ControllerPlanRevisionPersistenceProposal {
    pub fn from_controller_result(
        project_id: i64,
        parent: &PersistedPlan,
        source_review: &PlanReview,
        result: &ControllerPlanRevisionResult,
    ) -> Result<Self, ControllerPlanRevisionPersistenceProposalError> {
        if project_id <= 0 || parent.project_id != project_id {
            return Err(ControllerPlanRevisionPersistenceProposalError::InvalidProject);
        }
        result
            .validate()
            .map_err(|_| ControllerPlanRevisionPersistenceProposalError::InvalidRevisionResult)?;
        if parent.id != result.parent_plan_id
            || parent.version != result.parent_plan_version
            || parent.id <= 0
            || parent.version <= 0
            || parent.status != PlanStatus::RevisionRequested
            || parent.provenance.origin != PlanOrigin::Controller
            || parent.superseded_by_plan_id.is_some()
            || parent.response.validate().is_err()
        {
            return Err(ControllerPlanRevisionPersistenceProposalError::InvalidParent);
        }
        if source_review.id != result.review_id
            || source_review.plan_id != parent.id
            || source_review.origin != crate::storage::db::PlanReviewOrigin::Controller
            || source_review.decision != PlanReviewDecision::RevisePlan
            || source_review.superseded_by_review_id.is_some()
        {
            return Err(ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview);
        }
        let revision_feedback = persisted_revision_feedback(source_review)
            .map_err(|_| ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview)?;
        let proposal = Self {
            project_id,
            parent_plan_id: parent.id,
            parent_plan_version: parent.version,
            parent_plan: parent.response.clone(),
            source_review_id: source_review.id,
            source_review_details: source_review.details.clone(),
            revision_feedback,
            revised_plan: result.plan.clone(),
        };
        proposal.validate()?;
        Ok(proposal)
    }

    pub const fn project_id(&self) -> i64 {
        self.project_id
    }

    pub const fn parent_plan_id(&self) -> i64 {
        self.parent_plan_id
    }

    pub const fn parent_plan_version(&self) -> i64 {
        self.parent_plan_version
    }

    pub const fn source_review_id(&self) -> i64 {
        self.source_review_id
    }

    pub(crate) fn parent_plan(&self) -> &PlanResponse {
        &self.parent_plan
    }

    pub(crate) fn source_review_details(&self) -> &str {
        &self.source_review_details
    }

    pub(crate) fn revised_plan(&self) -> &PlanResponse {
        &self.revised_plan
    }

    pub fn validate(&self) -> Result<(), ControllerPlanRevisionPersistenceProposalError> {
        if self.project_id <= 0
            || self.parent_plan_id <= 0
            || self.parent_plan_version <= 0
            || self.source_review_id <= 0
        {
            return Err(ControllerPlanRevisionPersistenceProposalError::InvalidRevisionResult);
        }
        self.parent_plan
            .validate()
            .map_err(|_| ControllerPlanRevisionPersistenceProposalError::InvalidParent)?;
        self.revised_plan
            .validate()
            .map_err(|_| ControllerPlanRevisionPersistenceProposalError::InvalidRevisionResult)?;
        if self.source_review_details.trim().is_empty()
            || self.source_review_details.len() > MAX_REVIEW_DETAILS_BYTES
            || self.revision_feedback.trim().is_empty()
            || self.revision_feedback.len() > MAX_REVIEW_DETAILS_BYTES
        {
            return Err(ControllerPlanRevisionPersistenceProposalError::ReviewDetailsTooLarge);
        }
        let size = serde_json::to_vec(&PersistedRevisionSnapshot {
            project_id: self.project_id,
            parent_plan_id: self.parent_plan_id,
            parent_plan_version: self.parent_plan_version,
            source_review_id: self.source_review_id,
            parent_plan: &self.parent_plan,
            source_review_details: &self.source_review_details,
            revision_feedback: &self.revision_feedback,
            revised_plan: &self.revised_plan,
        })
        .map_err(|_| ControllerPlanRevisionPersistenceProposalError::RevisionTooLarge)?
        .len();
        if size > MAX_REVISION_PERSISTENCE_BYTES {
            return Err(ControllerPlanRevisionPersistenceProposalError::RevisionTooLarge);
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct PersistedRevisionSnapshot<'a> {
    project_id: i64,
    parent_plan_id: i64,
    parent_plan_version: i64,
    source_review_id: i64,
    parent_plan: &'a PlanResponse,
    source_review_details: &'a str,
    revision_feedback: &'a str,
    revised_plan: &'a PlanResponse,
}

/// Trusted application authorization for one exact revision proposal.
/// There is no public constructor, `Clone`, or serde implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct ControllerPlanRevisionPersistenceAuthorization {
    project_id: i64,
    parent_plan_id: i64,
    parent_plan_version: i64,
    parent_plan: PlanResponse,
    source_review_id: i64,
    source_review_details: String,
    revision_feedback: String,
    revised_plan: PlanResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanRevisionPersistenceAuthorizationRejection {
    Missing,
    NotAuthorizedForProposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanRevisionPersistenceFailure {
    InvalidProposal,
    CanonicalStorage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanRevisionPersistenceResult {
    AuthorizationRejected {
        reason: ControllerPlanRevisionPersistenceAuthorizationRejection,
    },
    FreshValidationRejected,
    PersistenceFailed {
        reason: ControllerPlanRevisionPersistenceFailure,
    },
    Persisted {
        plan_id: i64,
        version: i64,
        status: PlanStatus,
        origin: PlanOrigin,
        parent_plan_id: i64,
        source_review_id: i64,
    },
}

pub(crate) fn authorization_for(
    proposal: &ControllerPlanRevisionPersistenceProposal,
) -> ControllerPlanRevisionPersistenceAuthorization {
    ControllerPlanRevisionPersistenceAuthorization {
        project_id: proposal.project_id,
        parent_plan_id: proposal.parent_plan_id,
        parent_plan_version: proposal.parent_plan_version,
        parent_plan: proposal.parent_plan.clone(),
        source_review_id: proposal.source_review_id,
        source_review_details: proposal.source_review_details.clone(),
        revision_feedback: proposal.revision_feedback.clone(),
        revised_plan: proposal.revised_plan.clone(),
    }
}

pub(crate) fn matches_authorization(
    proposal: &ControllerPlanRevisionPersistenceProposal,
    authorization: &ControllerPlanRevisionPersistenceAuthorization,
) -> bool {
    authorization.project_id == proposal.project_id
        && authorization.parent_plan_id == proposal.parent_plan_id
        && authorization.parent_plan_version == proposal.parent_plan_version
        && authorization.parent_plan == proposal.parent_plan
        && authorization.source_review_id == proposal.source_review_id
        && authorization.source_review_details == proposal.source_review_details
        && authorization.revision_feedback == proposal.revision_feedback
        && authorization.revised_plan == proposal.revised_plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::OrcApp;
    use crate::controller_plan_revision::ControllerPlanRevisionResult;
    use crate::protocol::{PROTOCOL_VERSION, PlanResponse};

    fn plan(objective: &str) -> PlanResponse {
        PlanResponse {
            protocol_version: PROTOCOL_VERSION,
            objective: objective.into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        }
    }

    fn app_with_revision() -> (tempfile::TempDir, OrcApp, i64, PersistedPlan, i64) {
        let directory = tempfile::tempdir().unwrap();
        let orc_dir = directory.path().join(".orc");
        std::fs::create_dir_all(&orc_dir).unwrap();
        std::fs::write(orc_dir.join("engineering.md"), "Keep changes focused.\n").unwrap();
        let path = orc_dir.join("orc.db");
        let database = crate::storage::Database::init(&path).unwrap();
        let project_id = database
            .create_project("Controller revision persistence")
            .unwrap();
        let parent_id = database
            .store_controller_plan(project_id, &plan("original"))
            .unwrap();
        let parent = database.get_plan(parent_id).unwrap().unwrap();
        let review_details = serde_json::json!({
            "details": "The plan needs a concrete correction.",
            "revision_feedback": "Add the missing acceptance condition."
        })
        .to_string();
        let review_id = database
            .store_controller_plan_review(
                project_id,
                parent_id,
                parent.version,
                &parent.response,
                PlanReviewDecision::RevisePlan,
                &review_details,
            )
            .unwrap();
        let parent = database.get_plan(parent_id).unwrap().unwrap();
        drop(database);
        let app = OrcApp::open(&path, directory.path()).unwrap();
        (directory, app, project_id, parent, review_id)
    }

    fn result(
        parent: &PersistedPlan,
        review_id: i64,
        objective: &str,
    ) -> ControllerPlanRevisionResult {
        ControllerPlanRevisionResult {
            parent_plan_id: parent.id,
            parent_plan_version: parent.version,
            review_id,
            plan: plan(objective),
        }
    }

    fn review(app: &OrcApp, project_id: i64, review_id: i64) -> PlanReview {
        app.database()
            .list_plan_reviews(project_id)
            .unwrap()
            .into_iter()
            .find(|review| review.id == review_id)
            .unwrap()
    }

    fn proposal(
        app: &OrcApp,
        parent: &PersistedPlan,
        review_id: i64,
        objective: &str,
    ) -> ControllerPlanRevisionPersistenceProposal {
        app.propose_controller_plan_revision_persistence(&result(parent, review_id, objective))
            .unwrap()
    }

    #[test]
    fn successful_controller_revision_persists_one_next_version_atomically() {
        let (_directory, app, project_id, parent, review_id) = app_with_revision();
        let proposal = proposal(&app, &parent, review_id, "revised");
        let authorization = app.authorize_controller_plan_revision_persistence(&proposal);
        let result = app.execute_authorized_controller_plan_revision_persistence(
            &proposal,
            Some(authorization),
        );
        let new_id = match result {
            ControllerPlanRevisionPersistenceResult::Persisted {
                plan_id,
                version,
                status: PlanStatus::Proposed,
                origin: PlanOrigin::Controller,
                parent_plan_id,
                source_review_id,
            } => {
                assert_eq!(version, parent.version + 1);
                assert_eq!(parent_plan_id, parent.id);
                assert_eq!(source_review_id, review_id);
                plan_id
            }
            other => panic!("unexpected result: {other:?}"),
        };
        let parent_after = app.database().get_plan(parent.id).unwrap().unwrap();
        let new_plan = app.database().get_plan(new_id).unwrap().unwrap();
        assert_eq!(parent_after.status, PlanStatus::Cancelled);
        assert_eq!(parent_after.superseded_by_plan_id, Some(new_id));
        assert_eq!(new_plan.parent_plan_id, Some(parent.id));
        assert_eq!(
            new_plan.provenance,
            crate::storage::db::PlanProvenance::controller()
        );
        assert_eq!(new_plan.status, PlanStatus::Proposed);
        assert_eq!(
            app.database().list_plan_reviews(project_id).unwrap().len(),
            1
        );
        assert!(app.database().list_tasks().unwrap().is_empty());
        assert!(
            app.database()
                .list_lead_decisions(project_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            app.database()
                .list_agent_runs(project_id, usize::MAX)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn authorization_is_exact_one_shot_and_stale_state_is_rejected_without_extra_mutation() {
        let (_directory, app, project_id, parent, review_id) = app_with_revision();
        let first = proposal(&app, &parent, review_id, "first");
        let second = proposal(&app, &parent, review_id, "second");
        let authorization = app.authorize_controller_plan_revision_persistence(&first);
        assert!(matches!(
            app.execute_authorized_controller_plan_revision_persistence(&second, Some(authorization)),
            ControllerPlanRevisionPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanRevisionPersistenceAuthorizationRejection::NotAuthorizedForProposal
            }
        ));
        let authorization = app.authorize_controller_plan_revision_persistence(&first);
        app.database()
            .store_controller_plan(project_id, &plan("newer"))
            .unwrap();
        assert!(matches!(
            app.execute_authorized_controller_plan_revision_persistence(
                &first,
                Some(authorization)
            ),
            ControllerPlanRevisionPersistenceResult::FreshValidationRejected
        ));
        assert!(matches!(
            app.execute_authorized_controller_plan_revision_persistence(&first, None),
            ControllerPlanRevisionPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanRevisionPersistenceAuthorizationRejection::Missing
            }
        ));
        assert_eq!(
            app.database().list_plan_history(project_id).unwrap().len(),
            2
        );
    }

    #[test]
    fn non_revise_or_superseded_source_review_cannot_derive_a_proposal() {
        let (_directory, app, project_id, parent, review_id) = app_with_revision();
        let mut non_revise = review(&app, project_id, review_id);
        non_revise.decision = PlanReviewDecision::Approve;
        assert!(matches!(
            ControllerPlanRevisionPersistenceProposal::from_controller_result(
                project_id,
                &parent,
                &non_revise,
                &result(&parent, review_id, "not a revision"),
            ),
            Err(ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview)
        ));

        let mut superseded = review(&app, project_id, review_id);
        superseded.superseded_by_review_id = Some(review_id + 1);
        assert!(matches!(
            ControllerPlanRevisionPersistenceProposal::from_controller_result(
                project_id,
                &parent,
                &superseded,
                &result(&parent, review_id, "superseded"),
            ),
            Err(ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview)
        ));
        assert_eq!(
            app.database().list_plan_history(project_id).unwrap().len(),
            1
        );
    }

    #[test]
    fn invalid_or_substituted_revision_cannot_derive_or_persist() {
        let (_directory, app, project_id, parent, review_id) = app_with_revision();
        let mut invalid = result(&parent, review_id, "invalid");
        invalid.plan.protocol_version = PROTOCOL_VERSION + 1;
        let review = app
            .database()
            .list_plan_reviews(project_id)
            .unwrap()
            .into_iter()
            .find(|review| review.id == review_id)
            .unwrap();
        assert!(matches!(
            ControllerPlanRevisionPersistenceProposal::from_controller_result(
                project_id, &parent, &review, &invalid
            ),
            Err(ControllerPlanRevisionPersistenceProposalError::InvalidRevisionResult)
        ));
        let mut mismatched = result(&parent, review_id + 1, "mismatched");
        mismatched.parent_plan_id = parent.id;
        assert!(matches!(
            ControllerPlanRevisionPersistenceProposal::from_controller_result(
                project_id,
                &parent,
                &review,
                &mismatched
            ),
            Err(ControllerPlanRevisionPersistenceProposalError::InvalidSourceReview)
        ));
        assert_eq!(
            app.database().list_plan_history(project_id).unwrap().len(),
            1
        );
    }

    #[test]
    fn atomic_storage_failure_rolls_back_parent_and_new_plan() {
        let (_directory, app, project_id, parent, review_id) = app_with_revision();
        let mut revised = plan("duplicate dependency");
        let task = crate::protocol::TaskProposal {
            local_id: "child".into(),
            title: "Child".into(),
            objective: "Child objective".into(),
            role: "developer".into(),
            priority: crate::task::TaskPriority::Normal,
            depends_on: vec!["root".into(), "root".into()],
            capabilities: vec![],
            scope_mode: None,
            context_files: vec![],
            expected_changes: vec![],
            unchanged: vec![],
            acceptance_criteria: vec![],
            required_tests: vec![],
            validation: vec![],
            execution_hints: crate::protocol::ExecutionHints::default(),
            risk_factors: vec![],
        };
        let root = crate::protocol::TaskProposal {
            local_id: "root".into(),
            title: "Root".into(),
            objective: "Root objective".into(),
            role: "developer".into(),
            priority: crate::task::TaskPriority::Normal,
            depends_on: vec![],
            capabilities: vec![],
            scope_mode: None,
            context_files: vec![],
            expected_changes: vec![],
            unchanged: vec![],
            acceptance_criteria: vec![],
            required_tests: vec![],
            validation: vec![],
            execution_hints: crate::protocol::ExecutionHints::default(),
            risk_factors: vec![],
        };
        revised.tasks = vec![root, task];
        let details = serde_json::json!({
            "details": "The plan needs a concrete correction.",
            "revision_feedback": "Add the missing acceptance condition."
        })
        .to_string();
        let error = app.database().store_controller_plan_revision(
            project_id,
            parent.id,
            parent.version,
            &parent.response,
            review_id,
            &details,
            &revised,
        );
        assert!(error.is_err());
        assert_eq!(
            app.database().list_plan_history(project_id).unwrap().len(),
            1
        );
        assert_eq!(app.database().get_plan(parent.id).unwrap().unwrap(), parent);
        assert_eq!(
            app.database().list_plan_reviews(project_id).unwrap().len(),
            1
        );
    }
}
