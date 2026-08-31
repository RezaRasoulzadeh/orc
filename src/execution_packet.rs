//! Bounded, provider-independent execution packets.
//!
//! Collectors in this module turn deterministic Orc state into small role-specific
//! values. Rendering is deliberately separate from collection so tests and fake
//! transports can inspect exactly the same packet production transports receive.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::automated::{RevisionContract, RevisionTaskRequirements};
use crate::git::WorktreeChanges;
use crate::protocol::{PlanningProjectState, TaskRiskGuard};
use crate::storage::db::ReviewBlockerRecord;
use crate::task::{Task, TaskContract, TaskScopeMode};
use crate::validation::{ValidationCategory, ValidationReport};

pub const MAX_FILES: usize = 12;
pub const MAX_FILE_BYTES: usize = 12_000;
pub const MAX_TOTAL_FILE_BYTES: usize = 64_000;
pub const MAX_DIFF_BYTES: usize = 48_000;
pub const MAX_ENGINEERING_BYTES: usize = 32_000;
pub const MAX_DIAGNOSTIC_COMMANDS: usize = 6;
pub const MAX_DIAGNOSTIC_BYTES: usize = 6_000;
pub const MAX_BLOCKERS: usize = 12;
pub const MAX_LIST_ITEMS: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncation {
    pub field: String,
    pub omitted_items: usize,
    pub omitted_bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketMetadata {
    pub packet_type: String,
    pub included_file_count: usize,
    pub diff_bytes: usize,
    pub diagnostics_bytes: usize,
    pub blocker_count: usize,
    pub truncations: Vec<Truncation>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedText {
    pub text: String,
    pub original_bytes: usize,
    pub omitted_bytes: usize,
}

impl BoundedText {
    pub fn new(value: &str, limit: usize) -> Self {
        let original_bytes = value.len();
        if original_bytes <= limit {
            return Self {
                text: value.to_owned(),
                original_bytes,
                omitted_bytes: 0,
            };
        }
        let marker = "\n... [Orc omitted deterministic middle content] ...\n";
        if limit <= marker.len() {
            let end = floor_char_boundary(value, limit);
            return Self {
                text: value[..end].to_owned(),
                original_bytes,
                omitted_bytes: original_bytes - end,
            };
        }
        let available = limit.saturating_sub(marker.len());
        let head_limit = available * 2 / 3;
        let tail_limit = available - head_limit;
        let head_end = floor_char_boundary(value, head_limit);
        let tail_start = ceil_char_boundary(value, original_bytes.saturating_sub(tail_limit));
        let text = format!("{}{marker}{}", &value[..head_end], &value[tail_start..]);
        Self {
            omitted_bytes: original_bytes.saturating_sub(head_end + original_bytes - tail_start),
            text,
            original_bytes,
        }
    }

    pub fn truncated(&self) -> bool {
        self.omitted_bytes != 0
    }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(value: &str, mut index: usize) -> usize {
    index = index.min(value.len());
    while index < value.len() && !value.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskContractPacket {
    pub truncations: Vec<Truncation>,
    pub task_id: String,
    pub title: String,
    pub objective: String,
    pub role: String,
    pub scope_mode: Option<TaskScopeMode>,
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    pub unchanged: Vec<String>,
    pub expected_changes: Vec<String>,
    pub required_validation: Vec<String>,
}

impl TaskContractPacket {
    pub fn from_task(task: &Task, contract: &TaskContract) -> Self {
        let mut truncations = Vec::new();
        let mut list = |field: &str, values: &[String]| {
            let bounded = bounded_list(values);
            if values.len() > bounded.len() {
                truncations.push(Truncation {
                    field: format!("task_contract.{field}"),
                    omitted_items: values.len() - bounded.len(),
                    omitted_bytes: 0,
                });
            }
            let omitted_bytes = values
                .iter()
                .take(MAX_LIST_ITEMS)
                .zip(&bounded)
                .map(|(original, bounded)| original.len().saturating_sub(bounded.len()))
                .sum();
            if omitted_bytes != 0 {
                truncations.push(Truncation {
                    field: format!("task_contract.{field}"),
                    omitted_items: 0,
                    omitted_bytes,
                });
            }
            bounded
        };
        let acceptance_criteria = list("acceptance_criteria", &contract.acceptance_criteria);
        let required_tests = list("required_tests", &contract.required_tests);
        let unchanged = list("unchanged", &contract.unchanged);
        let expected_changes = list("expected_changes", &task.expected_changes);
        let required_validation = list("required_validation", &contract.validation);
        Self {
            truncations,
            task_id: task.id.clone(),
            title: task.title.clone(),
            objective: task.objective.clone(),
            role: task.role.clone(),
            scope_mode: task.scope_mode,
            acceptance_criteria,
            required_tests,
            unchanged,
            expected_changes,
            required_validation,
        }
    }

    fn from_revision(task: &Task, requirements: &RevisionTaskRequirements) -> Self {
        let contract = TaskContract {
            acceptance_criteria: requirements.acceptance_criteria.clone(),
            required_tests: requirements.required_tests.clone(),
            unchanged: requirements.unchanged.clone(),
            validation: requirements.validation.clone(),
        };
        let mut packet = Self::from_task(task, &contract);
        packet.expected_changes = bounded_list(&requirements.expected_changes);
        if requirements.expected_changes.len() > packet.expected_changes.len() {
            packet.truncations.push(Truncation {
                field: "task_contract.expected_changes".into(),
                omitted_items: requirements.expected_changes.len() - packet.expected_changes.len(),
                omitted_bytes: 0,
            });
        }
        packet
    }
}

fn bounded_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .take(MAX_LIST_ITEMS)
        .map(|value| BoundedText::new(value, 2_000).text)
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContext {
    pub path: String,
    pub selected_by: Vec<String>,
    pub content: BoundedText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeEvidence {
    pub files: Vec<crate::git::ChangedFile>,
    pub omitted_files: usize,
    pub stat: BoundedText,
    pub diff: BoundedText,
}

impl ChangeEvidence {
    pub fn from_worktree(changes: &WorktreeChanges) -> Self {
        let mut files = changes.files.clone();
        files.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.status.cmp(&right.status))
        });
        let omitted_files = files.len().saturating_sub(MAX_FILES * 2);
        files.truncate(MAX_FILES * 2);
        Self {
            files,
            omitted_files,
            stat: BoundedText::new(&changes.stat, 8_000),
            diff: BoundedText::new(&changes.diff, MAX_DIFF_BYTES),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchPacket {
    pub metadata: PacketMetadata,
    pub project: String,
    pub engineering_contract: BoundedText,
    pub task_contract: TaskContractPacket,
    pub risk_guards: Vec<TaskRiskGuard>,
    pub execution_plan: crate::worker_protocol::WorkerPlan,
    pub relevant_files: Vec<FileContext>,
    pub current_worktree: Option<ChangeEvidence>,
}

impl DispatchPacket {
    pub fn build(
        root: &Path,
        project: &str,
        engineering_contract: &str,
        task: &Task,
        contract: &TaskContract,
        plan: &crate::worker_protocol::WorkerPlan,
        changes: &WorktreeChanges,
    ) -> Result<Self> {
        let (relevant_files, mut truncations) = collect_files(
            root,
            &task.context_files,
            &task.expected_changes,
            changes,
            &[],
        )?;
        let engineering_contract = BoundedText::new(engineering_contract, MAX_ENGINEERING_BYTES);
        note_text(
            &mut truncations,
            "engineering_contract",
            &engineering_contract,
        );
        let current_worktree =
            (!changes.files.is_empty()).then(|| ChangeEvidence::from_worktree(changes));
        if let Some(evidence) = &current_worktree {
            note_text(&mut truncations, "current_worktree.diff", &evidence.diff);
            if evidence.omitted_files != 0 {
                truncations.push(Truncation {
                    field: "current_worktree.files".into(),
                    omitted_items: evidence.omitted_files,
                    omitted_bytes: 0,
                });
            }
        }
        let mut execution_plan = plan.clone();
        bound_plan_snapshot(
            &mut execution_plan,
            "See packet.current_worktree; Orc retained the full authoritative snapshot.",
            &mut truncations,
        );
        let metadata = PacketMetadata {
            packet_type: "dispatch".into(),
            included_file_count: relevant_files.len(),
            diff_bytes: current_worktree
                .as_ref()
                .map_or(0, |value| value.diff.text.len()),
            diagnostics_bytes: 0,
            blocker_count: 0,
            truncations,
        };
        Ok(Self {
            metadata,
            project: project.to_owned(),
            engineering_contract,
            task_contract: TaskContractPacket::from_task(task, contract),
            risk_guards: task.risk_policy().required_guards,
            execution_plan,
            relevant_files,
            current_worktree,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionPacket {
    pub metadata: PacketMetadata,
    pub project: String,
    pub engineering_contract: BoundedText,
    pub task_contract: TaskContractPacket,
    pub risk_guards: Vec<TaskRiskGuard>,
    pub active_blockers: Vec<ReviewBlockerRecord>,
    pub reviewer_feedback: Vec<String>,
    pub operator_feedback: Option<BoundedText>,
    pub current_changes: ChangeEvidence,
    pub relevant_files: Vec<FileContext>,
    pub execution_plan: crate::worker_protocol::WorkerPlan,
}

impl RevisionPacket {
    #[expect(
        clippy::too_many_arguments,
        reason = "explicit revision packet inputs document the invocation boundary"
    )]
    pub fn build(
        root: &Path,
        project: &str,
        engineering_contract: &str,
        task: &Task,
        revision: &RevisionContract,
        operator_feedback: Option<&str>,
        changes: &WorktreeChanges,
        plan: &crate::worker_protocol::WorkerPlan,
    ) -> Result<Self> {
        let mut blockers = if revision.active_blockers.is_empty() {
            revision
                .unresolved
                .iter()
                .chain(&revision.regressions)
                .filter(|blocker| blocker.status != "resolved")
                .cloned()
                .collect::<Vec<_>>()
        } else {
            revision
                .active_blockers
                .iter()
                .filter(|blocker| {
                    matches!(blocker.status.as_str(), "new" | "unresolved" | "regression")
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        blockers.sort_by(|left, right| left.blocker_id.cmp(&right.blocker_id));
        let omitted_blockers = blockers.len().saturating_sub(MAX_BLOCKERS);
        blockers.truncate(MAX_BLOCKERS);
        let blocker_text = blockers
            .iter()
            .flat_map(|blocker| {
                [
                    &blocker.requirement_ref,
                    &blocker.evidence,
                    &blocker.finding,
                ]
            })
            .cloned()
            .collect::<Vec<_>>();
        let (relevant_files, mut truncations) = collect_files(
            root,
            &task.context_files,
            &revision.original_task_requirements.expected_changes,
            changes,
            &blocker_text,
        )?;
        if omitted_blockers != 0 {
            truncations.push(Truncation {
                field: "active_blockers".into(),
                omitted_items: omitted_blockers,
                omitted_bytes: 0,
            });
        }
        let engineering_contract = BoundedText::new(engineering_contract, MAX_ENGINEERING_BYTES);
        note_text(
            &mut truncations,
            "engineering_contract",
            &engineering_contract,
        );
        let current_changes = ChangeEvidence::from_worktree(changes);
        note_text(
            &mut truncations,
            "current_changes.diff",
            &current_changes.diff,
        );
        if current_changes.omitted_files != 0 {
            truncations.push(Truncation {
                field: "current_changes.files".into(),
                omitted_items: current_changes.omitted_files,
                omitted_bytes: 0,
            });
        }
        let operator_feedback = operator_feedback
            .filter(|value| !value.trim().is_empty())
            .map(|value| BoundedText::new(value, 4_000));
        if let Some(value) = &operator_feedback {
            note_text(&mut truncations, "operator_feedback", value);
        }
        let mut execution_plan = plan.clone();
        bound_plan_snapshot(
            &mut execution_plan,
            "See packet.current_changes; Orc retained the full authoritative snapshot.",
            &mut truncations,
        );
        let selected_ids = blockers
            .iter()
            .map(|blocker| blocker.blocker_id.as_str())
            .collect::<BTreeSet<_>>();
        execution_plan
            .active_review_blockers
            .retain(|blocker| selected_ids.contains(blocker.id.as_str()));
        execution_plan.resolved_review_blockers.clear();
        for step in &mut execution_plan.steps {
            step.active_review_blockers
                .retain(|blocker| selected_ids.contains(blocker.as_str()));
        }
        let metadata = PacketMetadata {
            packet_type: "revision".into(),
            included_file_count: relevant_files.len(),
            diff_bytes: current_changes.diff.text.len(),
            diagnostics_bytes: 0,
            blocker_count: blockers.len(),
            truncations,
        };
        Ok(Self {
            metadata,
            project: project.to_owned(),
            engineering_contract,
            task_contract: TaskContractPacket::from_revision(
                task,
                &revision.original_task_requirements,
            ),
            risk_guards: task.risk_policy().required_guards,
            active_blockers: blockers,
            reviewer_feedback: bounded_list(&revision.reviewer_revision_feedback),
            operator_feedback,
            current_changes,
            relevant_files,
            execution_plan,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassingValidationEvidence {
    pub command: String,
    pub category: ValidationCategory,
    pub exit_status: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBlockerContext {
    pub blocker_id: String,
    pub status: String,
    pub acceptance_condition: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewPacket {
    pub metadata: PacketMetadata,
    pub task_contract: TaskContractPacket,
    pub risk_guards: Vec<TaskRiskGuard>,
    pub implementation_run_id: Option<i64>,
    pub current_changes: ChangeEvidence,
    pub passing_validation: Vec<PassingValidationEvidence>,
    pub prior_blockers: Vec<ReviewBlockerContext>,
}

/// A bounded advisory audit packet. Unlike [`ReviewPacket`], this is not an
/// acceptance gate and therefore carries no task-validation claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectReviewPacket {
    pub metadata: PacketMetadata,
    pub seed_task: TaskContractPacket,
    pub current_changes: ChangeEvidence,
}

impl ProjectReviewPacket {
    pub fn build(task: &Task, contract: &TaskContract, changes: &WorktreeChanges) -> Self {
        let current_changes = ChangeEvidence::from_worktree(changes);
        let mut truncations = Vec::new();
        note_text(
            &mut truncations,
            "current_changes.diff",
            &current_changes.diff,
        );
        if current_changes.omitted_files != 0 {
            truncations.push(Truncation {
                field: "current_changes.files".into(),
                omitted_items: current_changes.omitted_files,
                omitted_bytes: 0,
            });
        }
        Self {
            metadata: PacketMetadata {
                packet_type: "project_review".into(),
                included_file_count: current_changes.files.len(),
                diff_bytes: current_changes.diff.text.len(),
                diagnostics_bytes: 0,
                blocker_count: 0,
                truncations,
            },
            seed_task: TaskContractPacket::from_task(task, contract),
            current_changes,
        }
    }
}

impl ReviewPacket {
    pub fn build(
        task: &Task,
        contract: &TaskContract,
        run_id: Option<i64>,
        changes: &WorktreeChanges,
        validation: &ValidationReport,
        ledger: &[ReviewBlockerRecord],
    ) -> Result<Self> {
        if !validation.is_success() {
            bail!("review packet requires fresh passing deterministic validation evidence");
        }
        let current_changes = ChangeEvidence::from_worktree(changes);
        let mut prior_blockers = ledger
            .iter()
            .map(|blocker| ReviewBlockerContext {
                blocker_id: blocker.blocker_id.clone(),
                status: blocker.status.clone(),
                acceptance_condition: BoundedText::new(&blocker.acceptance_condition, 2_000).text,
            })
            .collect::<Vec<_>>();
        prior_blockers.sort_by(|left, right| left.blocker_id.cmp(&right.blocker_id));
        let omitted = prior_blockers.len().saturating_sub(MAX_BLOCKERS);
        prior_blockers.truncate(MAX_BLOCKERS);
        let mut truncations = Vec::new();
        note_text(
            &mut truncations,
            "current_changes.diff",
            &current_changes.diff,
        );
        if current_changes.omitted_files != 0 {
            truncations.push(Truncation {
                field: "current_changes.files".into(),
                omitted_items: current_changes.omitted_files,
                omitted_bytes: 0,
            });
        }
        if omitted != 0 {
            truncations.push(Truncation {
                field: "prior_blockers".into(),
                omitted_items: omitted,
                omitted_bytes: 0,
            });
        }
        Ok(Self {
            metadata: PacketMetadata {
                packet_type: "semantic_review".into(),
                included_file_count: current_changes.files.len(),
                diff_bytes: current_changes.diff.text.len(),
                diagnostics_bytes: 0,
                blocker_count: prior_blockers.len(),
                truncations,
            },
            task_contract: TaskContractPacket::from_task(task, contract),
            risk_guards: task.risk_policy().required_guards,
            implementation_run_id: run_id,
            current_changes,
            passing_validation: validation
                .steps
                .iter()
                .map(|step| PassingValidationEvidence {
                    command: step.command.clone(),
                    category: step.category,
                    exit_status: step.exit_status,
                })
                .collect(),
            prior_blockers,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedDiagnostic {
    pub command: String,
    pub category: ValidationCategory,
    pub exit_status: Option<i32>,
    pub classification: Option<crate::validation::ValidationFailureClassification>,
    pub output: BoundedText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationRepairPacket {
    pub metadata: PacketMetadata,
    pub task_id: String,
    pub objective: BoundedText,
    pub repair_attempt: usize,
    pub failures: Vec<FailedDiagnostic>,
    pub changed_files: Vec<crate::git::ChangedFile>,
    pub relevant_files: Vec<FileContext>,
}

impl ValidationRepairPacket {
    pub fn build(
        root: &Path,
        task: &Task,
        attempt: usize,
        report: &ValidationReport,
        changes: &WorktreeChanges,
    ) -> Result<Self> {
        let failed = report
            .steps
            .iter()
            .filter(|step| !step.passed)
            .collect::<Vec<_>>();
        let omitted_commands = failed.len().saturating_sub(MAX_DIAGNOSTIC_COMMANDS);
        let mut failures = Vec::new();
        let mut truncations = Vec::new();
        for step in failed.into_iter().take(MAX_DIAGNOSTIC_COMMANDS) {
            let output = BoundedText::new(&step.output(), MAX_DIAGNOSTIC_BYTES);
            note_text(
                &mut truncations,
                &format!("diagnostics:{}", step.command),
                &output,
            );
            failures.push(FailedDiagnostic {
                command: step.command.clone(),
                category: step.category,
                exit_status: step.exit_status,
                classification: step.failure_classification,
                output,
            });
        }
        if omitted_commands != 0 {
            truncations.push(Truncation {
                field: "failures".into(),
                omitted_items: omitted_commands,
                omitted_bytes: 0,
            });
        }
        let diagnostics_bytes = failures
            .iter()
            .map(|failure| failure.output.text.len())
            .sum();
        let empty: Vec<String> = Vec::new();
        let (relevant_files, file_truncations) = collect_files(
            root,
            &task.context_files,
            &task.expected_changes,
            changes,
            &empty,
        )?;
        truncations.extend(file_truncations);
        let mut changed_files = changes.files.clone();
        changed_files.sort_by(|left, right| left.path.cmp(&right.path));
        let omitted_changed_files = changed_files.len().saturating_sub(MAX_FILES * 2);
        changed_files.truncate(MAX_FILES * 2);
        if omitted_changed_files != 0 {
            truncations.push(Truncation {
                field: "changed_files".into(),
                omitted_items: omitted_changed_files,
                omitted_bytes: 0,
            });
        }
        Ok(Self {
            metadata: PacketMetadata {
                packet_type: "validation_repair".into(),
                included_file_count: relevant_files.len(),
                diff_bytes: 0,
                diagnostics_bytes,
                blocker_count: 0,
                truncations,
            },
            task_id: task.id.clone(),
            objective: BoundedText::new(&task.objective, 2_000),
            repair_attempt: attempt,
            failures,
            changed_files,
            relevant_files,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannerPacket {
    pub metadata: PacketMetadata,
    pub protocol_version: u32,
    pub kind: String,
    pub objective: BoundedText,
    pub project: Option<crate::protocol::ReportProject>,
    pub engineering_contract: BoundedText,
    pub constraints: Vec<String>,
    pub non_goals: Vec<String>,
    pub deliverables: Vec<String>,
    pub definition_of_done: Vec<String>,
    pub role_boundaries: Vec<String>,
    pub planning_constraints: Vec<String>,
    pub approval_requirements: Vec<String>,
    pub current_state: Option<PlanningProjectState>,
    pub discovery_snapshot: Option<crate::discovery::ProjectDiscoverySnapshot>,
    pub source_lead_decision: Vec<PlannerDecisionContext>,
    pub response_schema: crate::protocol::PlanResponseSchema,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannerDecisionContext {
    pub id: i64,
    pub kind: crate::lead::LeadDecisionKind,
    pub source_request: BoundedText,
    pub summary: BoundedText,
    pub details: BoundedText,
    pub resolution: Option<BoundedText>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeadPacket {
    pub metadata: PacketMetadata,
    pub request: BoundedText,
    pub project_id: i64,
    pub project_name: String,
    pub discovery: Option<crate::discovery::ProjectDiscoverySnapshot>,
    pub engineering_contract: BoundedText,
    pub architecture: Option<BoundedText>,
    pub facts: BTreeMap<String, String>,
    pub planning_state: PlanningProjectState,
    pub active_tasks: Vec<Task>,
    pub dependencies: BTreeMap<String, Vec<String>>,
    pub pending_approvals: Vec<crate::storage::db::ApprovalRequest>,
}

pub fn render_packet<T: Serialize>(role_instructions: &str, packet: &T) -> Result<String> {
    Ok(format!(
        "{role_instructions}\n\n## Authoritative Orc packet\n\n{}",
        serde_json::to_string_pretty(packet)?
    ))
}

fn note_text(truncations: &mut Vec<Truncation>, field: &str, value: &BoundedText) {
    if value.truncated() {
        truncations.push(Truncation {
            field: field.to_owned(),
            omitted_items: 0,
            omitted_bytes: value.omitted_bytes,
        });
    }
}

fn bound_plan_snapshot(
    plan: &mut crate::worker_protocol::WorkerPlan,
    replacement: &str,
    truncations: &mut Vec<Truncation>,
) {
    let original_bytes = plan.read_only_snapshot.len();
    plan.read_only_snapshot = replacement.into();
    if original_bytes > plan.read_only_snapshot.len() {
        truncations.push(Truncation {
            field: "execution_plan.read_only_snapshot".into(),
            omitted_items: 0,
            omitted_bytes: original_bytes - plan.read_only_snapshot.len(),
        });
    }
}

fn collect_files(
    root: &Path,
    context_paths: &[String],
    expected_paths: &[String],
    changes: &WorktreeChanges,
    reference_text: &[String],
) -> Result<(Vec<FileContext>, Vec<Truncation>)> {
    let mut candidates: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    add_paths(&mut candidates, root, context_paths, "task_context");
    add_paths(&mut candidates, root, expected_paths, "expected_change");
    let mut changed = changes
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    changed.sort();
    add_paths(&mut candidates, root, &changed, "current_change");
    for text in reference_text {
        let paths = referenced_paths(root, text);
        add_paths(&mut candidates, root, &paths, "blocker_reference");
    }

    let priorities = [
        "task_context",
        "expected_change",
        "current_change",
        "blocker_reference",
    ];
    let mut ordered = candidates.into_iter().collect::<Vec<_>>();
    ordered.sort_by(|(left_path, left_sources), (right_path, right_sources)| {
        let priority = |sources: &BTreeSet<String>| {
            priorities
                .iter()
                .position(|source| sources.contains(*source))
                .unwrap_or(priorities.len())
        };
        priority(left_sources)
            .cmp(&priority(right_sources))
            .then(left_path.cmp(right_path))
    });
    let omitted_files = ordered.len().saturating_sub(MAX_FILES);
    ordered.truncate(MAX_FILES);
    let mut files = Vec::new();
    let mut truncations = Vec::new();
    let mut remaining = MAX_TOTAL_FILE_BYTES;
    for (path, sources) in ordered {
        if remaining == 0 {
            truncations.push(Truncation {
                field: format!("relevant_files:{path}"),
                omitted_items: 1,
                omitted_bytes: 0,
            });
            continue;
        }
        let bytes = std::fs::read(root.join(&path))
            .with_context(|| format!("failed to read deterministic context file '{path}'"))?;
        let raw = match String::from_utf8(bytes) {
            Ok(value) => value,
            Err(error) => format!(
                "[binary or non-UTF-8 file: {} bytes]",
                error.as_bytes().len()
            ),
        };
        let content = BoundedText::new(&raw, MAX_FILE_BYTES.min(remaining));
        remaining = remaining.saturating_sub(content.text.len());
        note_text(
            &mut truncations,
            &format!("relevant_files:{path}"),
            &content,
        );
        files.push(FileContext {
            path,
            selected_by: sources.into_iter().collect(),
            content,
        });
    }
    if omitted_files != 0 {
        truncations.push(Truncation {
            field: "relevant_files".into(),
            omitted_items: omitted_files,
            omitted_bytes: 0,
        });
    }
    Ok((files, truncations))
}

fn add_paths(
    candidates: &mut BTreeMap<String, BTreeSet<String>>,
    root: &Path,
    paths: &[String],
    source: &str,
) {
    for path in paths {
        let path = path.trim().trim_matches(['`', '\'', '"']);
        if is_safe_file(root, path) {
            candidates
                .entry(path.replace('\\', "/"))
                .or_default()
                .insert(source.to_owned());
        }
    }
}

fn referenced_paths(root: &Path, text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|part| {
            part.trim_matches(|ch: char| {
                matches!(
                    ch,
                    '`' | '\'' | '"' | ',' | ':' | ';' | '(' | ')' | '[' | ']'
                )
            })
        })
        .filter(|part| part.contains('/') || part.contains('.'))
        .filter(|part| is_safe_file(root, part))
        .map(str::to_owned)
        .collect()
}

fn is_safe_file(root: &Path, path: &str) -> bool {
    let path = PathBuf::from(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        && root.join(path).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::ChangedFile;
    use crate::protocol::TaskRiskFactor;
    use crate::task::{TaskPriority, TaskStatus};
    use crate::validation::{ValidationFailureClassification, ValidationStepResult};

    fn task() -> Task {
        Task {
            id: "T-0042".into(),
            title: "Bound packets".into(),
            objective: "Make provider context deterministic".into(),
            role: "developer".into(),
            priority: TaskPriority::Normal,
            status: TaskStatus::Active,
            cancellation_reason: None,
            required_capabilities: vec!["code".into()],
            scope_mode: Some(TaskScopeMode::Focused),
            context_files: vec!["src/context.rs".into()],
            expected_changes: vec!["src/expected.rs".into()],
            reasoning_effort: Some(crate::registry::ReasoningEffort::Low),
            effort_reason: Some("bounded change".into()),
            risk_factors: vec![TaskRiskFactor::Persistence],
        }
    }

    fn contract() -> TaskContract {
        TaskContract {
            acceptance_criteria: vec!["packet is bounded".into()],
            required_tests: vec!["packet test".into()],
            unchanged: vec!["economy selection".into()],
            validation: vec!["cargo test".into()],
        }
    }

    fn plan() -> crate::worker_protocol::WorkerPlan {
        crate::worker_protocol::WorkerPlan {
            protocol_version: 1,
            read_only_snapshot: "snapshot".into(),
            unchanged: vec!["economy selection".into()],
            acceptance_criteria: Vec::new(),
            required_tests: Vec::new(),
            active_review_blockers: Vec::new(),
            resolved_review_blockers: Vec::new(),
            verification: Vec::new(),
            plan_acceptance_criteria: Vec::new(),
            plan_required_tests: Vec::new(),
            plan_review_blockers: Vec::new(),
            steps: Vec::new(),
        }
    }

    fn blocker(id: &str, status: &str, evidence: &str) -> ReviewBlockerRecord {
        ReviewBlockerRecord {
            task_id: "T-0042".into(),
            blocker_id: id.into(),
            run_id: 7,
            requirement_ref: "packet contract".into(),
            evidence: evidence.into(),
            severity: "high".into(),
            acceptance_condition: "bounded behavior is proved".into(),
            status: status.into(),
            finding: format!("finding {id}"),
            first_seen: String::new(),
            last_seen: String::new(),
            blocker_key: format!("key-{id}"),
        }
    }

    fn changes(diff: String) -> WorktreeChanges {
        WorktreeChanges {
            files: vec![ChangedFile {
                status: "M".into(),
                path: "src/changed.rs".into(),
            }],
            stat: "src/changed.rs | 1 +".into(),
            diff,
        }
    }

    fn validation(passed: bool, output: String) -> ValidationReport {
        ValidationReport {
            steps: vec![ValidationStepResult {
                command: "cargo test".into(),
                category: if passed {
                    ValidationCategory::Success
                } else {
                    ValidationCategory::Test
                },
                passed,
                stdout: if passed {
                    "old passed output".into()
                } else {
                    String::new()
                },
                stderr: if passed { String::new() } else { output },
                exit_status: Some(if passed { 0 } else { 1 }),
                diagnostics: None,
                failure_classification: (!passed)
                    .then_some(ValidationFailureClassification::Implementation),
                fallback_command: None,
            }],
        }
    }

    fn source_tree() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("src")).unwrap();
        for (path, value) in [
            ("context.rs", "context"),
            ("expected.rs", "expected"),
            ("changed.rs", "changed"),
            ("blocker.rs", "blocker"),
        ] {
            std::fs::write(directory.path().join("src").join(path), value).unwrap();
        }
        directory
    }

    #[test]
    fn dispatch_contains_contract_context_and_guards_but_no_review_history() {
        let root = source_tree();
        let packet = DispatchPacket::build(
            root.path(),
            "orc",
            "architecture",
            &task(),
            &contract(),
            &plan(),
            &changes("diff".into()),
        )
        .unwrap();
        let rendered = render_packet("stable instructions", &packet).unwrap();
        assert!(rendered.contains("packet is bounded"));
        assert!(rendered.contains("src/context.rs"));
        assert!(rendered.contains("persistence_reopen_coverage"));
        assert!(!rendered.contains("reviewer_feedback"));
        assert!(!rendered.contains("escalation"));
    }

    #[test]
    fn revision_keeps_only_active_blockers_in_stable_order() {
        let root = source_tree();
        let mut revision = RevisionContract {
            active_blockers: vec![
                blocker("BLK-z", "unresolved", "see src/blocker.rs"),
                blocker("BLK-resolved", "resolved", "old"),
                blocker("BLK-a", "regression", "current regression"),
            ],
            resolved_blockers: vec![blocker("BLK-old", "resolved", "history")],
            original_task_requirements: RevisionTaskRequirements {
                acceptance_criteria: contract().acceptance_criteria,
                required_tests: contract().required_tests,
                expected_changes: task().expected_changes,
                unchanged: contract().unchanged,
                validation: contract().validation,
            },
            ..Default::default()
        };
        revision.reviewer_revision_feedback = vec!["current feedback".into()];
        let packet = RevisionPacket::build(
            root.path(),
            "orc",
            "architecture",
            &task(),
            &revision,
            None,
            &changes("current diff".into()),
            &plan(),
        )
        .unwrap();
        assert_eq!(
            packet
                .active_blockers
                .iter()
                .map(|value| value.blocker_id.as_str())
                .collect::<Vec<_>>(),
            ["BLK-a", "BLK-z"]
        );
        let rendered = render_packet("revision", &packet).unwrap();
        assert!(!rendered.contains("BLK-resolved"));
        assert!(!rendered.contains("BLK-old"));
        assert!(
            packet
                .relevant_files
                .iter()
                .any(|file| file.path == "src/blocker.rs")
        );
    }

    #[test]
    fn review_contains_compact_current_evidence_and_refuses_failure() {
        let packet = ReviewPacket::build(
            &task(),
            &contract(),
            Some(9),
            &changes("authoritative diff".into()),
            &validation(true, String::new()),
            &[
                blocker("BLK-b", "unresolved", "e"),
                blocker("BLK-a", "resolved", "e"),
            ],
        )
        .unwrap();
        assert!(
            packet
                .current_changes
                .diff
                .text
                .contains("authoritative diff")
        );
        assert_eq!(packet.passing_validation[0].command, "cargo test");
        assert_eq!(packet.prior_blockers[0].blocker_id, "BLK-a");
        let rendered = render_packet("Do not execute validation commands", &packet).unwrap();
        assert!(!rendered.contains("old passed output"));
        assert!(
            ReviewPacket::build(
                &task(),
                &contract(),
                Some(9),
                &changes(String::new()),
                &validation(false, "failure".into()),
                &[],
            )
            .is_err()
        );
    }

    #[test]
    fn repair_contains_only_current_failures_and_bounded_diagnostics() {
        let root = source_tree();
        let mut report = validation(false, "x".repeat(MAX_DIAGNOSTIC_BYTES * 4));
        report.steps.push(ValidationStepResult {
            command: "cargo fmt --check".into(),
            category: ValidationCategory::Success,
            passed: true,
            stdout: "passed output must not enter repair".into(),
            stderr: String::new(),
            exit_status: Some(0),
            diagnostics: None,
            failure_classification: None,
            fallback_command: None,
        });
        let packet = ValidationRepairPacket::build(
            root.path(),
            &task(),
            2,
            &report,
            &changes("huge diff is not needed".into()),
        )
        .unwrap();
        assert_eq!(packet.failures.len(), 1);
        assert_eq!(packet.failures[0].command, "cargo test");
        assert!(packet.failures[0].output.text.len() <= MAX_DIAGNOSTIC_BYTES);
        assert!(packet.failures[0].output.omitted_bytes > 0);
        assert!(
            packet
                .metadata
                .truncations
                .iter()
                .any(|value| value.field.starts_with("diagnostics:"))
        );
        let rendered = render_packet("repair", &packet).unwrap();
        assert!(!rendered.contains("passed output must not enter repair"));
        assert!(!rendered.contains("huge diff is not needed"));
    }

    #[test]
    fn large_diff_and_many_blockers_are_bounded_with_visible_metadata() {
        let root = source_tree();
        let revision = RevisionContract {
            active_blockers: (0..MAX_BLOCKERS + 10)
                .rev()
                .map(|index| blocker(&format!("BLK-{index:03}"), "unresolved", "e"))
                .collect(),
            original_task_requirements: RevisionTaskRequirements {
                acceptance_criteria: contract().acceptance_criteria,
                required_tests: contract().required_tests,
                expected_changes: task().expected_changes,
                unchanged: contract().unchanged,
                validation: contract().validation,
            },
            ..Default::default()
        };
        let packet = RevisionPacket::build(
            root.path(),
            "orc",
            "architecture",
            &task(),
            &revision,
            None,
            &changes("d".repeat(MAX_DIFF_BYTES * 3)),
            &plan(),
        )
        .unwrap();
        assert_eq!(packet.active_blockers.len(), MAX_BLOCKERS);
        assert!(packet.current_changes.diff.text.len() <= MAX_DIFF_BYTES);
        assert!(
            packet
                .metadata
                .truncations
                .iter()
                .any(|value| value.field == "active_blockers" && value.omitted_items == 10)
        );
        assert!(
            packet
                .metadata
                .truncations
                .iter()
                .any(|value| value.field == "current_changes.diff" && value.omitted_bytes > 0)
        );
    }

    #[test]
    fn deterministic_selection_order_and_rendering_are_byte_stable() {
        let root = source_tree();
        let packet = DispatchPacket::build(
            root.path(),
            "orc",
            "architecture",
            &task(),
            &contract(),
            &plan(),
            &changes("diff".into()),
        )
        .unwrap();
        assert_eq!(
            packet
                .relevant_files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["src/context.rs", "src/expected.rs", "src/changed.rs"]
        );
        assert_eq!(
            render_packet("stable", &packet).unwrap(),
            render_packet("stable", &packet).unwrap()
        );
    }
}
