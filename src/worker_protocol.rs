//! Provider-independent Worker preparation and execution evidence.
//!
//! This module deliberately contains no provider or review policy.  It describes
//! the contract a worker must satisfy before it is allowed to execute.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannedOperation {
    Inspect,
    Create,
    Modify,
    Delete,
    Move,
    Command,
    Validate,
    NoMutation,
}

/// Derive the declared intent of an expected-change entry.  The prefix is a
/// task-contract convention, not a provider command, so plans remain portable.
pub fn operation_for_expected_change(value: &str) -> PlannedOperation {
    let value = value.trim().to_ascii_lowercase();
    if value.starts_with("create:") {
        PlannedOperation::Create
    } else if value.starts_with("delete:") {
        PlannedOperation::Delete
    } else if value.starts_with("move:") {
        PlannedOperation::Move
    } else if value.starts_with("inspect:") {
        PlannedOperation::Inspect
    } else if value.starts_with("command:") {
        PlannedOperation::Command
    } else if value.starts_with("validate:") {
        PlannedOperation::Validate
    } else if value.starts_with("no-mutation") {
        PlannedOperation::NoMutation
    } else {
        PlannedOperation::Modify
    }
}

pub fn operation_name(operation: &PlannedOperation) -> &'static str {
    match operation {
        PlannedOperation::Inspect => "inspect",
        PlannedOperation::Create => "create",
        PlannedOperation::Modify => "modify",
        PlannedOperation::Delete => "delete",
        PlannedOperation::Move => "move",
        PlannedOperation::Command => "command",
        PlannedOperation::Validate => "validate",
        PlannedOperation::NoMutation => "no_mutation",
    }
}

pub fn reported_operations(output: &str) -> Vec<PlannedOperation> {
    parse_reported_operations(output).unwrap_or_default()
}

/// Parse the protocol operation lines without silently dropping malformed
/// declarations.  A provider may return arbitrary narrative, but once it
/// starts the protocol prefix every such line must name a supported operation.
pub fn parse_reported_operations(output: &str) -> anyhow::Result<Vec<PlannedOperation>> {
    let mut operations = Vec::new();
    for line in output.lines() {
        let Some(value) = line.trim().strip_prefix("OPERATION PERFORMED: ") else {
            continue;
        };
        let value = value.trim();
        let operation = [
            PlannedOperation::Inspect,
            PlannedOperation::Create,
            PlannedOperation::Modify,
            PlannedOperation::Delete,
            PlannedOperation::Move,
            PlannedOperation::Command,
            PlannedOperation::Validate,
            PlannedOperation::NoMutation,
        ]
        .into_iter()
        .find(|operation| operation_name(operation) == value)
        .ok_or_else(|| anyhow::anyhow!("unknown planned operation '{value}'"))?;
        operations.push(operation);
    }
    if operations.is_empty()
        && let Some(values) = serde_json::from_str::<serde_json::Value>(output)
            .ok()
            .and_then(|value| {
                value
                    .get("worker_protocol")
                    .and_then(|protocol| protocol.get("operations_performed"))
                    .cloned()
            })
            .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
    {
        for value in values {
            let operation = [
                PlannedOperation::Inspect,
                PlannedOperation::Create,
                PlannedOperation::Modify,
                PlannedOperation::Delete,
                PlannedOperation::Move,
                PlannedOperation::Command,
                PlannedOperation::Validate,
                PlannedOperation::NoMutation,
            ]
            .into_iter()
            .find(|operation| operation_name(operation) == value)
            .ok_or_else(|| anyhow::anyhow!("unknown planned operation '{value}'"))?;
            operations.push(operation);
        }
    }
    Ok(operations)
}

pub fn reported_verifications(output: &str) -> Vec<String> {
    let mut checks = output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("VERIFICATION PASSED: ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .collect::<Vec<_>>();
    if checks.is_empty()
        && let Some(values) = serde_json::from_str::<serde_json::Value>(output)
            .ok()
            .and_then(|value| {
                value
                    .get("worker_protocol")
                    .and_then(|protocol| protocol.get("verification_passed"))
                    .cloned()
            })
            .and_then(|value| serde_json::from_value::<Vec<String>>(value).ok())
    {
        checks = values;
    }
    checks
}

/// A requirement identity is local to one persisted task contract.  The text
/// is retained beside the identity so an execution record remains useful when
/// inspected without reopening the task row.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRequirement {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewBlockerRequirement {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedStep {
    pub id: String,
    #[serde(default)]
    pub objective: String,
    pub intent: String,
    pub operations: Vec<PlannedOperation>,
    #[serde(default)]
    pub operation_targets: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    #[serde(default)]
    pub active_review_blockers: Vec<String>,
    pub verification: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerPlan {
    pub protocol_version: u32,
    pub read_only_snapshot: String,
    pub unchanged: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<WorkerRequirement>,
    #[serde(default)]
    pub required_tests: Vec<WorkerRequirement>,
    #[serde(default)]
    pub active_review_blockers: Vec<ReviewBlockerRequirement>,
    #[serde(default)]
    pub resolved_review_blockers: Vec<String>,
    #[serde(default)]
    pub verification: Vec<String>,
    pub steps: Vec<PlannedStep>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepEvidence {
    pub step_id: String,
    /// Observations produced while checking this step (command output, a
    /// worktree inspection, or another concrete result).  The plan's
    /// requested checks are not evidence by themselves.
    pub observed: Vec<String>,
    pub verification: Vec<String>,
    pub passed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerExecutionResult {
    pub protocol_version: u32,
    pub performed_operations: Vec<PlannedOperation>,
    pub affected_files: Vec<String>,
    pub requirement_coverage: Vec<(String, String)>,
    pub focused_verification: Vec<StepEvidence>,
    pub configured_validation: Vec<String>,
    pub unresolved_issues: Vec<String>,
}

fn normalized(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn requirement_ids(values: &[WorkerRequirement], kind: &str) -> anyhow::Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    for requirement in values {
        if requirement.id.trim().is_empty() || requirement.text.trim().is_empty() {
            anyhow::bail!("{kind} requirement IDs and text must not be empty");
        }
        if !ids.insert(requirement.id.clone()) {
            anyhow::bail!("{kind} requirement IDs must be unique");
        }
    }
    Ok(ids)
}

impl WorkerPlan {
    /// Upgrade the v1 persisted shape produced before requirement identities
    /// and explicit targets were added. This is only for historical records;
    /// all newly prepared plans are constructed in the strict shape.
    pub fn upgrade_legacy(&mut self) {
        if self.acceptance_criteria.is_empty() {
            let mut values = Vec::new();
            for step in &self.steps {
                for value in &step.acceptance_criteria {
                    if !values.iter().any(|known: &String| known == value) {
                        values.push(value.clone());
                    }
                }
            }
            self.acceptance_criteria = values
                .iter()
                .enumerate()
                .map(|(index, text)| WorkerRequirement {
                    id: format!("legacy-acceptance-{}", index + 1),
                    text: text.clone(),
                })
                .collect();
            for step in &mut self.steps {
                step.acceptance_criteria = step
                    .acceptance_criteria
                    .iter()
                    .filter_map(|value| {
                        values
                            .iter()
                            .position(|known| known == value)
                            .map(|index| format!("legacy-acceptance-{}", index + 1))
                    })
                    .collect();
            }
        }
        if self.required_tests.is_empty() {
            let mut values = Vec::new();
            for step in &self.steps {
                for value in &step.required_tests {
                    if !values.iter().any(|known: &String| known == value) {
                        values.push(value.clone());
                    }
                }
            }
            self.required_tests = values
                .iter()
                .enumerate()
                .map(|(index, text)| WorkerRequirement {
                    id: format!("legacy-required-test-{}", index + 1),
                    text: text.clone(),
                })
                .collect();
            for step in &mut self.steps {
                step.required_tests = step
                    .required_tests
                    .iter()
                    .filter_map(|value| {
                        values
                            .iter()
                            .position(|known| known == value)
                            .map(|index| format!("legacy-required-test-{}", index + 1))
                    })
                    .collect();
            }
        }
        if self.verification.is_empty() {
            for step in &self.steps {
                for value in &step.verification {
                    if !self.verification.contains(value) {
                        self.verification.push(value.clone());
                    }
                }
            }
        }
        for step in &mut self.steps {
            if step.objective.trim().is_empty() {
                step.objective = step.intent.clone();
            }
            if step.operation_targets.is_empty() {
                let target = step.intent.split_once(':').map_or_else(
                    || {
                        if step.intent.eq_ignore_ascii_case("no-mutation") {
                            "worktree".to_owned()
                        } else {
                            step.intent.trim().to_owned()
                        }
                    },
                    |(_, target)| target.trim().to_owned(),
                );
                step.operation_targets = vec![target];
            }
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            anyhow::bail!(
                "unsupported worker protocol version {}",
                self.protocol_version
            );
        }
        if self.read_only_snapshot.trim().is_empty() {
            anyhow::bail!("PREPARE must record a read-only project snapshot");
        }
        if self.steps.is_empty() {
            anyhow::bail!("PREPARE must contain at least one ordered step");
        }
        let mut ids = BTreeSet::new();
        let criteria = requirement_ids(&self.acceptance_criteria, "acceptance")?;
        let tests = requirement_ids(&self.required_tests, "required test")?;
        let mut blockers = BTreeSet::new();
        for blocker in &self.active_review_blockers {
            if blocker.id.trim().is_empty() || blocker.text.trim().is_empty() {
                anyhow::bail!("active review blocker IDs and text must not be empty")
            }
            if !blockers.insert(blocker.id.clone()) {
                anyhow::bail!("active review blocker IDs must be unique");
            }
        }
        let unchanged: BTreeSet<_> = self.unchanged.iter().map(|v| v.trim()).collect();
        if self.verification.is_empty()
            || self
                .verification
                .iter()
                .any(|value| value.trim().is_empty())
        {
            anyhow::bail!("PREPARE must declare concrete verification evidence");
        }
        let resolved: BTreeSet<_> = self
            .resolved_review_blockers
            .iter()
            .map(|value| value.trim())
            .collect();
        if resolved.iter().any(|value| value.is_empty()) {
            anyhow::bail!("resolved review blocker IDs must not be empty");
        }
        if resolved.iter().any(|value| blockers.contains(*value)) {
            anyhow::bail!("a review blocker cannot be both active and resolved");
        }
        for step in &self.steps {
            if step.id.trim().is_empty() || !ids.insert(step.id.clone()) {
                anyhow::bail!("PREPARE contains a duplicate or empty step id");
            }
            if step.objective.trim().is_empty()
                || step.intent.trim().is_empty()
                || step.operations.is_empty()
                || step.operation_targets.len() != step.operations.len()
                || step.verification.is_empty()
            {
                anyhow::bail!(
                    "step '{}' is missing objective, operation target, operation, or verification",
                    step.id
                );
            }
            if step.verification.iter().any(|value| {
                value.trim().is_empty() || !self.verification.iter().any(|check| check == value)
            }) {
                anyhow::bail!("step '{}' declares unknown verification evidence", step.id);
            }
            if step
                .acceptance_criteria
                .iter()
                .any(|value| !criteria.contains(value))
                || step
                    .required_tests
                    .iter()
                    .any(|value| !tests.contains(value))
                || step
                    .active_review_blockers
                    .iter()
                    .any(|value| !blockers.contains(value))
            {
                anyhow::bail!("step '{}' references an unknown requirement", step.id);
            }
            if step
                .operations
                .iter()
                .any(|op| matches!(op, PlannedOperation::NoMutation))
                && step.operations.len() != 1
            {
                anyhow::bail!(
                    "NoMutation must be the only operation in step '{}'",
                    step.id
                );
            }
            if step
                .operation_targets
                .iter()
                .any(|target| target.trim().is_empty())
            {
                anyhow::bail!("step '{}' contains an empty operation target", step.id);
            }
        }
        if self.acceptance_criteria.is_empty() {
            anyhow::bail!("PREPARE omits acceptance criteria");
        }
        if self.required_tests.is_empty() {
            anyhow::bail!("PREPARE omits required tests");
        }
        let covered_criteria: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|step| step.acceptance_criteria.iter())
            .collect();
        if self
            .acceptance_criteria
            .iter()
            .any(|requirement| !covered_criteria.contains(&requirement.id))
        {
            anyhow::bail!("PREPARE omits acceptance criterion coverage");
        }
        let covered_tests: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|step| step.required_tests.iter())
            .collect();
        if self
            .required_tests
            .iter()
            .any(|requirement| !covered_tests.contains(&requirement.id))
        {
            anyhow::bail!("PREPARE omits required test coverage");
        }
        for requirement in self
            .acceptance_criteria
            .iter()
            .chain(self.required_tests.iter())
        {
            if unchanged
                .iter()
                .any(|constraint| normalized(constraint) == normalized(&requirement.text))
            {
                anyhow::bail!("PREPARE contradicts an unchanged constraint");
            }
        }
        for step in &self.steps {
            if step.objective.trim().is_empty()
                || unchanged
                    .iter()
                    .any(|constraint| normalized(constraint) == normalized(&step.objective))
                || unchanged
                    .iter()
                    .any(|constraint| normalized(constraint) == normalized(&step.intent))
                || step.operation_targets.iter().any(|target| {
                    unchanged
                        .iter()
                        .any(|constraint| normalized(constraint) == normalized(target))
                })
            {
                anyhow::bail!("PREPARE contradicts an unchanged constraint");
            }
        }
        Ok(())
    }

    /// Validate the plan against the contract which was persisted when the task was created.
    pub fn validate_contract(
        &self,
        acceptance: &[WorkerRequirement],
        tests: &[WorkerRequirement],
        unchanged: &[String],
    ) -> anyhow::Result<()> {
        self.validate()?;
        if self.acceptance_criteria != acceptance {
            anyhow::bail!("PREPARE acceptance criteria do not match the authoritative task");
        }
        if self.required_tests != tests {
            anyhow::bail!("PREPARE required tests do not match the authoritative task");
        }
        if self.unchanged != unchanged {
            anyhow::bail!("PREPARE unchanged constraints do not match the authoritative task");
        }
        let covered: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|s| s.acceptance_criteria.iter().map(|v| v.trim()))
            .collect();
        let tested: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|s| s.required_tests.iter().map(|v| v.trim()))
            .collect();
        for value in acceptance {
            if !covered.contains(value.id.trim()) {
                anyhow::bail!("PREPARE omits acceptance criterion '{}'", value.id);
            }
        }
        for value in tests {
            if !tested.contains(value.id.trim()) {
                anyhow::bail!("PREPARE omits required test '{}'", value.id);
            }
        }
        let blocker_ids: BTreeSet<_> = self
            .active_review_blockers
            .iter()
            .map(|value| value.id.as_str())
            .collect();
        let covered_blockers: BTreeSet<_> = self
            .steps
            .iter()
            .flat_map(|step| step.active_review_blockers.iter().map(String::as_str))
            .collect();
        if blocker_ids != covered_blockers {
            anyhow::bail!("PREPARE omits one or more active review blockers");
        }
        let unchanged: BTreeSet<_> = unchanged.iter().map(|v| v.trim()).collect();
        if acceptance.iter().chain(tests.iter()).any(|requirement| {
            unchanged
                .iter()
                .any(|constraint| normalized(constraint) == normalized(&requirement.text))
        }) {
            anyhow::bail!("PREPARE contradicts an unchanged constraint");
        }
        Ok(())
    }
}

impl WorkerExecutionResult {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            anyhow::bail!("unsupported execution protocol version");
        }
        if self.focused_verification.iter().any(|e| !e.passed) {
            anyhow::bail!("a declared step verification failed");
        }
        for evidence in &self.focused_verification {
            if evidence.step_id.trim().is_empty()
                || evidence.observed.is_empty()
                || evidence.observed.iter().any(|v| v.trim().is_empty())
                || evidence.verification.is_empty()
                || evidence.verification.iter().any(|v| v.trim().is_empty())
            {
                anyhow::bail!(
                    "step evidence must identify a step and contain concrete verification"
                );
            }
            if !evidence.observed.iter().any(|value| {
                value.contains("worktree inspection") || value.contains("validation observed")
            }) {
                anyhow::bail!("step evidence must contain a post-step observation");
            }
            for check in &evidence.verification {
                if !evidence
                    .observed
                    .iter()
                    .any(|value| value == &format!("verification passed: {check}"))
                {
                    anyhow::bail!(
                        "step evidence does not independently observe verification '{}',",
                        check
                    );
                }
            }
        }
        if self.focused_verification.is_empty() {
            anyhow::bail!("execution must record focused verification evidence");
        }
        Ok(())
    }

    pub fn validate_against_plan(&self, plan: &WorkerPlan) -> anyhow::Result<()> {
        self.validate()?;
        let expected: Vec<_> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        let actual: Vec<_> = self
            .focused_verification
            .iter()
            .map(|e| e.step_id.as_str())
            .collect();
        if expected != actual {
            anyhow::bail!("execution evidence does not cover the prepared steps");
        }
        let operations: Vec<_> = plan
            .steps
            .iter()
            .flat_map(|s| s.operations.clone())
            .collect();
        if self.performed_operations != operations {
            anyhow::bail!("performed operations do not match the prepared plan order");
        }
        let covered: BTreeSet<_> = self
            .requirement_coverage
            .iter()
            .map(|(r, _)| r.trim())
            .collect();
        for requirement in plan.steps.iter().flat_map(|s| {
            s.acceptance_criteria
                .iter()
                .chain(s.required_tests.iter())
                .chain(s.active_review_blockers.iter())
        }) {
            if !covered.contains(requirement.trim()) {
                anyhow::bail!(
                    "execution evidence omits planned requirement '{}',",
                    requirement
                );
            }
        }
        if self
            .requirement_coverage
            .iter()
            .any(|(_, step)| !expected.contains(&step.as_str()))
        {
            anyhow::bail!("execution evidence contains an unknown step");
        }
        for (step, evidence) in plan.steps.iter().zip(&self.focused_verification) {
            if evidence.verification != step.verification {
                anyhow::bail!(
                    "verification evidence does not match step '{}'; it must be observed for the declared checks",
                    step.id
                );
            }
        }
        if self.requirement_coverage.iter().any(|(requirement, step)| {
            !plan.steps.iter().any(|candidate| {
                candidate.id == *step
                    && candidate
                        .acceptance_criteria
                        .iter()
                        .chain(candidate.required_tests.iter())
                        .chain(candidate.active_review_blockers.iter())
                        .any(|value| value == requirement)
            })
        }) {
            anyhow::bail!("execution evidence contains invalid requirement coverage");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan(op: PlannedOperation) -> WorkerPlan {
        WorkerPlan {
            protocol_version: 1,
            read_only_snapshot: "clean".into(),
            unchanged: vec![],
            acceptance_criteria: vec![WorkerRequirement {
                id: "acceptance-criterion-1".into(),
                text: "works".into(),
            }],
            required_tests: vec![WorkerRequirement {
                id: "required-test-1".into(),
                text: "test".into(),
            }],
            active_review_blockers: vec![],
            resolved_review_blockers: vec![],
            verification: vec!["check".into()],
            steps: vec![PlannedStep {
                id: "s1".into(),
                objective: "do task".into(),
                intent: "do task".into(),
                operations: vec![op],
                operation_targets: vec!["worktree".into()],
                acceptance_criteria: vec!["acceptance-criterion-1".into()],
                required_tests: vec!["required-test-1".into()],
                active_review_blockers: vec![],
                verification: vec!["check".into()],
            }],
        }
    }
    #[test]
    fn valid_operations_are_accepted() {
        for op in [
            PlannedOperation::Create,
            PlannedOperation::Modify,
            PlannedOperation::Delete,
            PlannedOperation::Move,
            PlannedOperation::Inspect,
            PlannedOperation::Command,
            PlannedOperation::Validate,
            PlannedOperation::NoMutation,
        ] {
            assert!(plan(op).validate().is_ok());
        }
    }
    #[test]
    fn incomplete_and_contradictory_plans_are_rejected() {
        let mut p = plan(PlannedOperation::Inspect);
        p.steps[0].required_tests.clear();
        assert!(p.validate().is_err());
        let mut p = plan(PlannedOperation::Modify);
        p.unchanged = vec!["works".into()];
        assert!(p.validate().is_err());
    }
    #[test]
    fn failed_evidence_is_not_success() {
        let r = WorkerExecutionResult {
            protocol_version: 1,
            performed_operations: vec![],
            affected_files: vec![],
            requirement_coverage: vec![],
            focused_verification: vec![StepEvidence {
                step_id: "s".into(),
                observed: vec!["observed failure".into()],
                verification: vec!["no".into()],
                passed: false,
            }],
            configured_validation: vec![],
            unresolved_issues: vec![],
        };
        assert!(r.validate().is_err());
    }

    #[test]
    fn requirement_ids_are_deterministic_and_complete_coverage_is_required() {
        let p = plan(PlannedOperation::Inspect);
        assert_eq!(p.acceptance_criteria[0].id, "acceptance-criterion-1");
        assert_eq!(p.required_tests[0].id, "required-test-1");
        assert!(
            p.validate_contract(&p.acceptance_criteria, &p.required_tests, &[])
                .is_ok()
        );

        let mut missing_acceptance = p.clone();
        missing_acceptance.steps[0].acceptance_criteria.clear();
        assert!(
            missing_acceptance
                .validate_contract(&p.acceptance_criteria, &p.required_tests, &[])
                .is_err()
        );

        let mut missing_test = p.clone();
        missing_test.steps[0].required_tests.clear();
        assert!(
            missing_test
                .validate_contract(&p.acceptance_criteria, &p.required_tests, &[])
                .is_err()
        );
    }

    #[test]
    fn active_blockers_and_verification_are_mandatory_but_resolved_are_preserve_only() {
        let mut p = plan(PlannedOperation::Inspect);
        p.active_review_blockers = vec![ReviewBlockerRequirement {
            id: "BLK-1".into(),
            text: "the issue stays fixed".into(),
        }];
        p.resolved_review_blockers = vec!["BLK-resolved".into()];
        p.steps[0].active_review_blockers = vec!["BLK-1".into()];
        assert!(
            p.validate_contract(&p.acceptance_criteria, &p.required_tests, &[])
                .is_ok()
        );

        let mut missing_blocker = p.clone();
        missing_blocker.steps[0].active_review_blockers.clear();
        assert!(
            missing_blocker
                .validate_contract(
                    &missing_blocker.acceptance_criteria,
                    &missing_blocker.required_tests,
                    &[]
                )
                .is_err()
        );

        let mut missing_verification = p.clone();
        missing_verification.verification.clear();
        assert!(missing_verification.validate().is_err());
    }

    #[test]
    fn operation_targets_and_unchanged_constraints_are_validated() {
        let mut p = plan(PlannedOperation::Modify);
        p.steps[0].operation_targets = vec![];
        assert!(p.validate().is_err());

        let mut contradictory = plan(PlannedOperation::Modify);
        contradictory.unchanged = vec!["worktree".into()];
        assert!(contradictory.validate().is_err());
    }

    #[test]
    fn historical_plan_shape_is_upgraded_without_losing_step_order() {
        let mut p = plan(PlannedOperation::Inspect);
        p.acceptance_criteria.clear();
        p.required_tests.clear();
        p.verification.clear();
        p.steps[0].objective.clear();
        p.steps[0].operation_targets.clear();
        p.steps[0].acceptance_criteria = vec!["works".into()];
        p.steps[0].required_tests = vec!["test".into()];
        p.upgrade_legacy();
        assert_eq!(p.steps[0].id, "s1");
        assert!(p.validate().is_ok());
    }
}
