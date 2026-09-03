//! Trusted, supervised persistence of a Controller Plan-review result.
//!
//! Controller review judgment remains separate from this mutation boundary:
//! only a validated result can derive a proposal, and only trusted
//! application code can mint the one-shot authorization consumed by execution.

use crate::controller_plan_review::{ControllerPlanReviewDecision, ControllerPlanReviewResult};
use crate::protocol::PlanResponse;
use crate::storage::db::{
    PersistedPlan, PlanOrigin, PlanReviewDecision, PlanReviewOrigin, PlanStatus,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_PERSISTED_DETAILS_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ControllerPlanReviewPersistenceProposalError {
    #[error("Controller Plan review project identity is invalid")]
    InvalidProject,
    #[error("Controller Plan review target is not a Controller-origin Plan")]
    InvalidPlanOrigin,
    #[error("Controller Plan review Plan identity is invalid")]
    InvalidPlanIdentity,
    #[error("Controller Plan review result is invalid")]
    InvalidControllerResult,
    #[error("Controller Plan review details exceed the persistence bound")]
    DetailsTooLarge,
}

/// A private, exact snapshot of one Controller review proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerPlanReviewPersistenceProposal {
    project_id: i64,
    workflow_id: Option<i64>,
    operator_resolution: Option<String>,
    plan_id: i64,
    plan_version: i64,
    plan: PlanResponse,
    decision: ControllerPlanReviewDecision,
    details: String,
    revision_feedback: Option<String>,
}

impl ControllerPlanReviewPersistenceProposal {
    /// Derives a proposal from a validated Controller result and trusted
    /// canonical Plan data. This performs no persistence or status mutation.
    pub fn from_controller_result(
        project_id: i64,
        plan: &PersistedPlan,
        result: &ControllerPlanReviewResult,
    ) -> Result<Self, ControllerPlanReviewPersistenceProposalError> {
        Self::from_controller_result_for_workflow(project_id, None, None, plan, result)
    }

    pub fn from_controller_result_for_workflow(
        project_id: i64,
        workflow_id: Option<i64>,
        operator_resolution: Option<&str>,
        plan: &PersistedPlan,
        result: &ControllerPlanReviewResult,
    ) -> Result<Self, ControllerPlanReviewPersistenceProposalError> {
        if project_id <= 0 || project_id != plan.project_id {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidProject);
        }
        if workflow_id.is_some_and(|id| id <= 0) {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidProject);
        }
        let operator_resolution = operator_resolution.map(str::to_owned);
        if plan.provenance.origin != PlanOrigin::Controller {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidPlanOrigin);
        }
        if plan.id <= 0 || plan.version <= 0 {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity);
        }
        result
            .validate()
            .map_err(|_| ControllerPlanReviewPersistenceProposalError::InvalidControllerResult)?;
        let proposal = Self {
            project_id,
            workflow_id,
            operator_resolution,
            plan_id: plan.id,
            plan_version: plan.version,
            plan: plan.response.clone(),
            decision: result.decision,
            details: result.details.clone(),
            revision_feedback: result.revision_feedback.clone(),
        };
        proposal.validate()?;
        Ok(proposal)
    }

    pub const fn project_id(&self) -> i64 {
        self.project_id
    }

    pub const fn workflow_id(&self) -> Option<i64> {
        self.workflow_id
    }

    pub fn operator_resolution(&self) -> Option<&str> {
        self.operator_resolution.as_deref()
    }

    pub const fn plan_id(&self) -> i64 {
        self.plan_id
    }

    pub const fn plan_version(&self) -> i64 {
        self.plan_version
    }

    pub const fn decision(&self) -> ControllerPlanReviewDecision {
        self.decision
    }

    /// Deterministic proposal validation repeated immediately before use.
    pub fn validate(&self) -> Result<(), ControllerPlanReviewPersistenceProposalError> {
        if self.project_id <= 0
            || self.plan_id <= 0
            || self.plan_version <= 0
            || self.plan.protocol_version == 0
        {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity);
        }
        if self.workflow_id.is_some_and(|id| id <= 0) {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidProject);
        }
        if self
            .operator_resolution
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(ControllerPlanReviewPersistenceProposalError::InvalidPlanIdentity);
        }
        self.plan
            .validate()
            .map_err(|_| ControllerPlanReviewPersistenceProposalError::InvalidControllerResult)?;
        validate_details(&self.details)?;
        if let Some(feedback) = &self.revision_feedback {
            validate_details(feedback)?;
        }
        if self.persisted_details()?.len() > MAX_PERSISTED_DETAILS_BYTES {
            return Err(ControllerPlanReviewPersistenceProposalError::DetailsTooLarge);
        }
        Ok(())
    }

    pub(crate) fn plan(&self) -> &PlanResponse {
        &self.plan
    }

    pub(crate) fn persisted_details(
        &self,
    ) -> Result<String, ControllerPlanReviewPersistenceProposalError> {
        serde_json::to_string(&PersistedControllerReviewDetails {
            details: &self.details,
            revision_feedback: self.revision_feedback.as_deref(),
        })
        .map_err(|_| ControllerPlanReviewPersistenceProposalError::DetailsTooLarge)
    }

    fn database_decision(&self) -> PlanReviewDecision {
        match self.decision {
            ControllerPlanReviewDecision::Approve => PlanReviewDecision::Approve,
            ControllerPlanReviewDecision::RevisePlan => PlanReviewDecision::RevisePlan,
            ControllerPlanReviewDecision::OperatorDecisionRequired => {
                PlanReviewDecision::UserDecisionRequired
            }
        }
    }
}

#[derive(Serialize)]
struct PersistedControllerReviewDetails<'a> {
    details: &'a str,
    revision_feedback: Option<&'a str>,
}

fn validate_details(value: &str) -> Result<(), ControllerPlanReviewPersistenceProposalError> {
    if value.trim().is_empty() || value.len() > 2048 {
        return Err(ControllerPlanReviewPersistenceProposalError::DetailsTooLarge);
    }
    Ok(())
}

/// Trusted application authorization for exactly one project/Plan/review
/// proposal. It has no public constructor, Clone, or serde implementation.
#[derive(Debug, PartialEq, Eq)]
pub struct ControllerPlanReviewPersistenceAuthorization {
    project_id: i64,
    workflow_id: Option<i64>,
    operator_resolution: Option<String>,
    plan_id: i64,
    plan_version: i64,
    plan: PlanResponse,
    decision: ControllerPlanReviewDecision,
    details: String,
    revision_feedback: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanReviewPersistenceAuthorizationRejection {
    Missing,
    NotAuthorizedForProposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanReviewPersistenceFailure {
    InvalidProposal,
    CanonicalStorage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerPlanReviewPersistenceResult {
    AuthorizationRejected {
        reason: ControllerPlanReviewPersistenceAuthorizationRejection,
    },
    FreshValidationRejected,
    PersistenceFailed {
        reason: ControllerPlanReviewPersistenceFailure,
    },
    Persisted {
        review_id: i64,
        plan_id: i64,
        origin: PlanReviewOrigin,
        decision: ControllerPlanReviewDecision,
        plan_status: PlanStatus,
    },
}

pub(crate) fn authorization_for(
    proposal: &ControllerPlanReviewPersistenceProposal,
) -> ControllerPlanReviewPersistenceAuthorization {
    ControllerPlanReviewPersistenceAuthorization {
        project_id: proposal.project_id,
        workflow_id: proposal.workflow_id,
        operator_resolution: proposal.operator_resolution.clone(),
        plan_id: proposal.plan_id,
        plan_version: proposal.plan_version,
        plan: proposal.plan.clone(),
        decision: proposal.decision,
        details: proposal.details.clone(),
        revision_feedback: proposal.revision_feedback.clone(),
    }
}

pub(crate) fn matches_authorization(
    proposal: &ControllerPlanReviewPersistenceProposal,
    authorization: &ControllerPlanReviewPersistenceAuthorization,
) -> bool {
    authorization.project_id == proposal.project_id
        && authorization.workflow_id == proposal.workflow_id
        && authorization.operator_resolution == proposal.operator_resolution
        && authorization.plan_id == proposal.plan_id
        && authorization.plan_version == proposal.plan_version
        && authorization.plan == proposal.plan
        && authorization.decision == proposal.decision
        && authorization.details == proposal.details
        && authorization.revision_feedback == proposal.revision_feedback
}

pub(crate) fn database_decision(
    proposal: &ControllerPlanReviewPersistenceProposal,
) -> PlanReviewDecision {
    proposal.database_decision()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::OrcApp;
    use crate::protocol::PROTOCOL_VERSION;

    fn result(decision: ControllerPlanReviewDecision) -> ControllerPlanReviewResult {
        ControllerPlanReviewResult {
            decision,
            details: "bounded review details".into(),
            revision_feedback: (decision == ControllerPlanReviewDecision::RevisePlan)
                .then_some("bounded revision feedback".into()),
        }
    }

    fn app_with_plan() -> (tempfile::TempDir, OrcApp, i64, PersistedPlan) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("review.sqlite");
        let database = crate::storage::Database::init(&path).unwrap();
        let project_id = database
            .create_project("Controller review persistence")
            .unwrap();
        let response = crate::protocol::PlanResponse {
            protocol_version: PROTOCOL_VERSION,
            objective: "persist one review".into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        };
        let plan_id = database
            .store_controller_plan(project_id, &response)
            .unwrap();
        let plan = database.get_plan(plan_id).unwrap().unwrap();
        drop(database);
        let app = OrcApp::open(&path, directory.path()).unwrap();
        (directory, app, project_id, plan)
    }

    #[test]
    fn all_three_controller_decisions_persist_exact_status_and_provenance() {
        for (decision, expected_status) in [
            (ControllerPlanReviewDecision::Approve, PlanStatus::Approved),
            (
                ControllerPlanReviewDecision::RevisePlan,
                PlanStatus::RevisionRequested,
            ),
            (
                ControllerPlanReviewDecision::OperatorDecisionRequired,
                PlanStatus::UnderReview,
            ),
        ] {
            let (_directory, app, project_id, plan) = app_with_plan();
            let proposal = app
                .propose_controller_plan_review_persistence(plan.id, &result(decision))
                .unwrap();
            let authorization = app.authorize_controller_plan_review_persistence(&proposal);
            let execution = app.execute_authorized_controller_plan_review_persistence(
                &proposal,
                Some(authorization),
            );
            let review_id = match execution {
                ControllerPlanReviewPersistenceResult::Persisted {
                    review_id,
                    origin: PlanReviewOrigin::Controller,
                    decision: actual,
                    plan_status,
                    ..
                } => {
                    assert_eq!(actual, decision);
                    assert_eq!(plan_status, expected_status);
                    review_id
                }
                other => panic!("unexpected result: {other:?}"),
            };
            let database = app.database();
            let persisted = database.get_plan(plan.id).unwrap().unwrap();
            assert_eq!(persisted.status, expected_status);
            let reviews = database.list_plan_reviews(project_id).unwrap();
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0].id, review_id);
            assert_eq!(reviews[0].origin, PlanReviewOrigin::Controller);
            assert_eq!(reviews[0].decision, proposal.database_decision());
            assert!(reviews[0].lead_run_id.is_none());
            assert!(reviews[0].lead_decision_id.is_none());
            assert!(database.list_tasks().unwrap().is_empty());
            assert!(
                database
                    .list_agent_runs(project_id, usize::MAX)
                    .unwrap()
                    .is_empty()
            );
            assert!(database.list_lead_decisions(project_id).unwrap().is_empty());
            assert!(app.active_workflow().unwrap().is_none());
        }
    }

    #[test]
    fn authorization_is_exact_and_one_shot() {
        let (_directory, app, project_id, plan) = app_with_plan();
        let first = ControllerPlanReviewPersistenceProposal::from_controller_result(
            project_id,
            &plan,
            &result(ControllerPlanReviewDecision::Approve),
        )
        .unwrap();
        let second = ControllerPlanReviewPersistenceProposal::from_controller_result(
            project_id,
            &plan,
            &result(ControllerPlanReviewDecision::RevisePlan),
        )
        .unwrap();
        let authorization = app.authorize_controller_plan_review_persistence(&first);
        assert!(matches!(
            app.execute_authorized_controller_plan_review_persistence(&second, Some(authorization)),
            ControllerPlanReviewPersistenceResult::AuthorizationRejected {
                reason:
                    ControllerPlanReviewPersistenceAuthorizationRejection::NotAuthorizedForProposal
            }
        ));
        let authorization = app.authorize_controller_plan_review_persistence(&first);
        assert!(matches!(
            app.execute_authorized_controller_plan_review_persistence(&first, Some(authorization)),
            ControllerPlanReviewPersistenceResult::Persisted { .. }
        ));
        assert!(matches!(
            app.execute_authorized_controller_plan_review_persistence(&first, None),
            ControllerPlanReviewPersistenceResult::AuthorizationRejected {
                reason: ControllerPlanReviewPersistenceAuthorizationRejection::Missing
            }
        ));
        assert_eq!(
            app.database().list_plan_reviews(project_id).unwrap().len(),
            1
        );
    }

    #[test]
    fn stale_plan_and_invalid_proposal_are_rejected_without_mutation() {
        let (_directory, app, project_id, plan) = app_with_plan();
        let proposal = ControllerPlanReviewPersistenceProposal::from_controller_result(
            project_id,
            &plan,
            &result(ControllerPlanReviewDecision::Approve),
        )
        .unwrap();
        let authorization = app.authorize_controller_plan_review_persistence(&proposal);
        app.database()
            .store_controller_plan(project_id, &plan.response)
            .unwrap();
        assert!(matches!(
            app.execute_authorized_controller_plan_review_persistence(
                &proposal,
                Some(authorization)
            ),
            ControllerPlanReviewPersistenceResult::FreshValidationRejected
        ));
        assert!(
            app.database()
                .list_plan_reviews(project_id)
                .unwrap()
                .is_empty()
        );
        assert!(app.database().list_tasks().unwrap().is_empty());
    }

    #[test]
    fn invalid_controller_result_cannot_derive_a_persistence_proposal() {
        let (_directory, app, project_id, plan) = app_with_plan();
        let invalid = ControllerPlanReviewResult {
            decision: ControllerPlanReviewDecision::RevisePlan,
            details: String::new(),
            revision_feedback: Some("feedback".into()),
        };
        assert!(matches!(
            ControllerPlanReviewPersistenceProposal::from_controller_result(
                project_id, &plan, &invalid
            ),
            Err(ControllerPlanReviewPersistenceProposalError::InvalidControllerResult)
        ));
        assert!(
            app.database()
                .list_plan_reviews(project_id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            app.database().get_plan(plan.id).unwrap().unwrap().status,
            PlanStatus::Proposed
        );
    }
}
