use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::lead::{LeadBackend, LeadBackendResponse, LeadContext, LeadResponse, LeadService};
use crate::protocol::{PlanResponse, PlanningRequest};
use crate::registry::{AgentAction, AgentDefinition, ReasoningEffort, ResolutionRecord};
use crate::review::ReviewSummary;
use crate::storage::{AgentRunExecution, Database};
use crate::validation::ValidationRunner;
use crate::worker::{TokenUsage, WorkerOutcome};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ActionOverrides {
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedAction {
    pub action: AgentAction,
    pub agent: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub resolution_record: ResolutionRecord,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionExecution {
    pub output: String,
    pub token_usage: Option<TokenUsage>,
}

pub struct ActionProgress<'a> {
    pub schema: &'a str,
    pub callback: &'a dyn Fn(&str),
}

pub fn resolve_action(
    db: &Database,
    action: AgentAction,
    overrides: &ActionOverrides,
) -> Result<(AgentDefinition, ResolvedAction)> {
    resolve_action_with_transport(
        db,
        action,
        overrides,
        crate::scheduler::TransportEligibility::Strict,
        &crate::scheduler::ProviderQuotaRefresher,
    )
}

fn resolve_action_with_transport(
    db: &Database,
    action: AgentAction,
    overrides: &ActionOverrides,
    transport: crate::scheduler::TransportEligibility,
    quota_refresher: &dyn crate::scheduler::QuotaRefresher,
) -> Result<(AgentDefinition, ResolvedAction)> {
    let decision = crate::scheduler::resolve_action_economy_for_execution_with_refresher(
        db,
        action,
        crate::scheduler::EconomyOverrides {
            agent_id: overrides.agent_id.clone(),
            model: overrides.model.clone(),
            effort: overrides.reasoning_effort,
        },
        transport,
        quota_refresher,
    )?;
    let resolution = decision.resolution.ok_or_else(|| {
        anyhow::anyhow!(
            "no eligible agent supports action '{}': {}",
            action.as_str(),
            decision.schedule.explanation
        )
    })?;
    let agent = resolution.agent;
    let resolved = ResolvedAction {
        action,
        agent: agent.id.clone(),
        model: resolution.execution.model,
        reasoning_effort: resolution.execution.reasoning_effort,
        resolution_record: resolution.record,
    };
    Ok((agent, resolved))
}

pub trait ActionBackend {
    fn transport_eligibility(&self) -> crate::scheduler::TransportEligibility {
        crate::scheduler::TransportEligibility::IgnoreUnsupportedBackend
    }

    fn quota_refresher(&self) -> &dyn crate::scheduler::QuotaRefresher {
        &crate::scheduler::UnsupportedQuotaRefresher
    }

    fn invoke(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<ActionExecution>;

    fn invoke_with_progress(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
        _progress: ActionProgress<'_>,
    ) -> Result<ActionExecution> {
        self.invoke(agent, action, input, model, effort)
    }

    fn observe(&self, _message: &str) {}
}

pub struct WorkerActionBackend {
    repo: PathBuf,
    planner_executable: Option<PathBuf>,
}

/// The Planner execution boundary exposes only planning to its provider.
/// This prevents a planner implementation from reaching orchestration actions
/// even when it shares the general action backend infrastructure.
pub struct PlannerActionBackend<'a> {
    inner: &'a dyn ActionBackend,
}

impl<'a> PlannerActionBackend<'a> {
    pub fn new(inner: &'a dyn ActionBackend) -> Self {
        Self { inner }
    }
}

impl ActionBackend for PlannerActionBackend<'_> {
    fn observe(&self, message: &str) {
        self.inner.observe(message);
    }

    fn invoke(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<ActionExecution> {
        if action != AgentAction::Plan {
            bail!(
                "Planner boundary rejects orchestration action '{}'",
                action.as_str()
            );
        }
        self.inner.invoke(agent, action, input, model, effort)
    }

    fn invoke_with_progress(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
        progress: ActionProgress<'_>,
    ) -> Result<ActionExecution> {
        if action != AgentAction::Plan {
            bail!(
                "Planner boundary rejects orchestration action '{}'",
                action.as_str()
            );
        }
        self.inner
            .invoke_with_progress(agent, action, input, model, effort, progress)
    }
}

impl WorkerActionBackend {
    pub fn new(repo: impl AsRef<Path>) -> Self {
        Self {
            repo: repo.as_ref().to_path_buf(),
            planner_executable: None,
        }
    }

    #[cfg(test)]
    fn with_planner_executable(mut self, executable: impl AsRef<Path>) -> Self {
        self.planner_executable = Some(executable.as_ref().to_path_buf());
        self
    }
}

impl ActionBackend for WorkerActionBackend {
    fn transport_eligibility(&self) -> crate::scheduler::TransportEligibility {
        crate::scheduler::TransportEligibility::Strict
    }

    fn quota_refresher(&self) -> &dyn crate::scheduler::QuotaRefresher {
        if self.planner_executable.is_some() {
            &crate::scheduler::UnsupportedQuotaRefresher
        } else {
            &crate::scheduler::ProviderQuotaRefresher
        }
    }

    fn invoke(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<ActionExecution> {
        self.invoke_with_progress(
            agent,
            action,
            input,
            model,
            effort,
            ActionProgress {
                schema: "{}",
                callback: &|_| {},
            },
        )
    }

    fn invoke_with_progress(
        &self,
        agent: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
        progress: ActionProgress<'_>,
    ) -> Result<ActionExecution> {
        let worker = match action {
            AgentAction::Lead | AgentAction::Plan => {
                crate::backend::WorkerFactory::build_read_only(
                    agent,
                    model.map(str::to_owned),
                    effort,
                    self.planner_executable.clone(),
                )
            }
            AgentAction::Review => {
                crate::backend::WorkerFactory::build_review(agent, model.map(str::to_owned), effort)
            }
            _ => crate::backend::WorkerFactory::build_with_overrides(
                agent,
                model.map(str::to_owned),
                effort,
            ),
        }
        .map_err(anyhow::Error::msg)?;
        let review_dir = (action == AgentAction::Review)
            .then(review_execution_directory)
            .transpose()?;
        let working_dir = review_dir.as_deref().unwrap_or(&self.repo);
        let execution = worker
            .execute_structured_with_progress_and_usage(
                input,
                working_dir,
                progress.schema,
                &|event| {
                    (progress.callback)(&worker.activity(event));
                },
            )
            .map_err(anyhow::Error::msg);
        if let Some(directory) = review_dir {
            let _ = std::fs::remove_dir_all(directory);
        }
        let execution = execution?;
        match execution.outcome {
            WorkerOutcome::Success => Ok(ActionExecution {
                output: execution
                    .output
                    .context("provider completed without structured output")?,
                token_usage: execution.token_usage,
            }),
            WorkerOutcome::Failure(error) => bail!(error),
        }
    }

    fn observe(&self, message: &str) {
        if normal_provider_progress(message).is_some() {
            eprintln!("{message}");
        }
    }
}

fn review_execution_directory() -> Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "orc-review-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).with_context(|| {
        format!(
            "failed to create isolated review directory {}",
            directory.display()
        )
    })?;
    Ok(directory)
}

fn normal_provider_progress(message: &str) -> Option<&str> {
    (!message.starts_with("provider item.")
        && !message.starts_with("provider turn.")
        && message != "provider activity")
        .then_some(message)
}

fn schema(action: AgentAction) -> String {
    let string_array = serde_json::json!({"type":"array","items":{"type":"string"}});
    let planned_task = serde_json::json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "local_id":{"type":"string"},"title":{"type":"string"},"objective":{"type":"string"},
            "role":{"type":"string"},"priority":{"type":"string","enum":["low","normal","high","critical"]},
            "depends_on":string_array,"capabilities":string_array,
            "scope_mode":{"type":["string","null"],"enum":["focused","module","project",null]},"context_files":string_array,
            "expected_changes":{"type":"array","minItems":1,"maxItems":crate::protocol::TaskProposal::MAX_EXPECTED_CHANGES,"items":{"type":"string","minLength":1}},
            "unchanged":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
            "acceptance_criteria":{"type":"array","minItems":1,"maxItems":crate::protocol::TaskProposal::MAX_ACCEPTANCE_CRITERIA,"items":{"type":"string","minLength":1}},
            "required_tests":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
            "validation":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
            "execution_hints":{"type":"object","additionalProperties":false,"properties":{"class":{"type":["string","null"],"enum":["coder","reviewer","architect","researcher","general",null]},"model":{"type":["string","null"],"minLength":1},"effort":{"type":"string","enum":["low","medium","high"]},"effort_reason":{"type":"string","minLength":1,"maxLength":240}},"required":["class","model","effort","effort_reason"]},
            "risk_factors":{"type":"array","items":{"type":"string","enum":["state_machine_lifecycle","persistence","restart_recovery","concurrency","cross_role_protocol","schema_data_flow","verification"]}}
        },
        "required":["local_id","title","objective","role","priority","depends_on","capabilities","scope_mode","context_files","expected_changes","unchanged","acceptance_criteria","required_tests","validation","execution_hints","risk_factors"]
    });
    let plan = serde_json::json!({
        "type":"object","additionalProperties":false,
        "properties":{"protocol_version":{"type":"integer"},"objective":{"type":"string"},"assumptions":string_array,"risks":string_array,"questions":string_array,"tasks":{"type":"array","items":planned_task}},
        "required":["protocol_version","objective","assumptions","risks","questions","tasks"]
    });
    let decision_details = serde_json::json!({
        "type":"object","additionalProperties":false,
        "properties":{
            "tasks":{"type":"array","items":planned_task},
            "objective":{"type":"string"},"reason":{"type":"string"},
            "question":{"type":"string"},"options":string_array,
            "feedback":{"type":"string"},"summary":{"type":"string"},
            "operator":{"type":"string"},"next":{"type":"string"}
        },
        "required":[]
    });
    let decision = serde_json::json!({
        "type":"object","additionalProperties":false,
        "properties":{
            "kind":{"type":"string","enum":["DIRECT_TASKS","PLAN_REQUIRED","USER_DECISION_REQUIRED","APPROVE","REVISE_PLAN"]},
            "details":decision_details
        },
        "required":["kind","details"]
    });
    let value = match action {
        AgentAction::Review => serde_json::json!({
            "type":"object","additionalProperties":false,
            "properties":{"verdict":{"type":"string","enum":["PASS","REVISE","REJECT"]},"findings":string_array,"blocking_findings":string_array,"non_blocking_findings":string_array,"severity":{"type":["string","null"]},"revision_feedback":{"type":["string","null"]},"blockers":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"prior_blocker_id":{"type":["string","null"]},"blocker_key":{"type":"string","minLength":1},"requirement_ref":{"type":"string"},"evidence":{"type":"string"},"severity":{"type":"string","enum":["low","medium","high","critical","unspecified"]},"acceptance_condition":{"type":"string"},"status":{"type":"string","enum":["new","unresolved","resolved","regression"]},"finding":{"type":"string"}},"required":["id","prior_blocker_id","blocker_key","requirement_ref","evidence","severity","acceptance_condition","status","finding"]}}},
            "required":["verdict","findings","blocking_findings","non_blocking_findings","severity","revision_feedback","blockers"]
        }),
        AgentAction::Plan => plan,
        AgentAction::Lead => serde_json::json!({
            "type":"object","additionalProperties":false,
            "properties":{"message":{"type":"string"},"proposals":{"type":"array","items":{"oneOf":[
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"plan"},"details":plan},"required":["kind","details"]},
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"task"},"details":planned_task},"required":["kind","details"]},
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"revision"},"details":{"type":"object","additionalProperties":false,"properties":{"task_id":{"type":"string"},"feedback":{"type":"string"}},"required":["task_id","feedback"]}},"required":["kind","details"]},
                {"type":"object","additionalProperties":false,"properties":{"kind":{"const":"approval_request"},"details":{"type":"object","additionalProperties":false,"properties":{"reason":{"type":"string"},"details":{"type":"string"}},"required":["reason","details"]}},"required":["kind","details"]}
            ]}},"decision":decision},
            "required":["message","proposals","decision"]
        }),
        AgentAction::Code => serde_json::json!({"type":"object"}),
    };
    value.to_string()
}

/// Native provider schema for the structured result returned by a revision worker.
pub fn revision_handoff_schema() -> String {
    let completion: serde_json::Value =
        serde_json::from_str(&crate::worker_protocol::plan_completion_schema())
            .expect("canonical Worker completion schema is valid JSON");
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "completion": completion,
            "claims": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "blocker_id": {"type": "string"},
                        "status": {"type": "string", "enum": ["addressed", "unresolved"]},
                        "implementation_summary": {"type": "string"},
                        "changed_files": {"type": "array", "items": {"type": "string"}},
                        "unresolved_risk": {"type": ["string", "null"]}
                    },
                    "required": [
                        "blocker_id",
                        "status",
                        "implementation_summary",
                        "changed_files",
                        "unresolved_risk"
                    ]
                }
            }
        },
        "required": ["completion", "claims"]
    })
    .to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewResult {
    pub verdict: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub blocking_findings: Vec<String>,
    #[serde(default)]
    pub non_blocking_findings: Vec<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub revision_feedback: Option<String>,
    #[serde(default)]
    pub blockers: Vec<ReviewBlocker>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReviewBlocker {
    pub id: String,
    #[serde(default)]
    pub prior_blocker_id: Option<String>,
    pub blocker_key: String,
    pub requirement_ref: String,
    pub evidence: String,
    pub severity: String,
    pub acceptance_condition: String,
    pub status: String,
    pub finding: String,
}

/// The actionable work contract handed from a review to a revision worker.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevisionContract {
    /// Blockers which must be implemented and proved by this revision.
    #[serde(default)]
    pub active_blockers: Vec<crate::storage::db::ReviewBlockerRecord>,
    /// Blockers which are already resolved. These are preserve-only
    /// invariants and are never revision work items.
    #[serde(default)]
    pub resolved_blockers: Vec<crate::storage::db::ReviewBlockerRecord>,
    #[serde(default)]
    pub reviewer_revision_feedback: Vec<String>,
    #[serde(default)]
    pub original_task_requirements: RevisionTaskRequirements,
    #[serde(default)]
    pub current_persisted_execution_evidence: Option<crate::worker_protocol::WorkerExecutionResult>,
    #[serde(default)]
    pub validation_failures: Vec<String>,
    // These fields retain the v1 ledger shape for persisted contracts and
    // inspection clients. The new fields above are the authoritative shape.
    #[serde(default)]
    pub unresolved: Vec<crate::storage::db::ReviewBlockerRecord>,
    #[serde(default)]
    pub regressions: Vec<crate::storage::db::ReviewBlockerRecord>,
    #[serde(default)]
    pub regression_constraints: Vec<crate::storage::db::ReviewBlockerRecord>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevisionTaskRequirements {
    pub acceptance_criteria: Vec<String>,
    pub required_tests: Vec<String>,
    pub expected_changes: Vec<String>,
    pub unchanged: Vec<String>,
    pub validation: Vec<String>,
}

impl RevisionContract {
    fn active_blocker_records(&self) -> Vec<&crate::storage::db::ReviewBlockerRecord> {
        if self.active_blockers.is_empty() {
            self.unresolved
                .iter()
                .chain(self.regressions.iter())
                .collect()
        } else {
            self.active_blockers.iter().collect()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionClaim {
    pub blocker_id: String,
    pub status: String,
    pub implementation_summary: String,
    pub changed_files: Vec<String>,
    pub unresolved_risk: Option<String>,
}

/// Task-specific validation captured by Orc after the current
/// diff was inspected. The fingerprint ties the report to the exact worktree
/// state it applies to, so a stale report cannot validate a changed
/// implementation. Validation ownership belongs to Orc's dispatch/revision
/// pipeline, not to the provider sessions that produce the implementation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionValidationEvidence {
    pub evidence_id: String,
    pub worktree_fingerprint: String,
    pub report: crate::validation::ValidationReport,
}

pub fn revision_worktree_fingerprint(changes: &crate::git::WorktreeChanges) -> String {
    let value = serde_json::to_vec(changes).expect("worktree changes are serializable");
    let mut hash: u64 = 14695981039346656037;
    for byte in value {
        hash = (hash ^ u64::from(byte)).wrapping_mul(1099511628211);
    }
    format!("rev-{hash:016x}")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionHandoff {
    /// Canonical Worker completion. Optional only to read provider results
    /// persisted before the unified completion contract was introduced.
    #[serde(default)]
    pub completion: Option<crate::worker_protocol::ReportedPlanCompletion>,
    pub claims: Vec<RevisionClaim>,
}

pub fn validate_revision_handoff(
    contract: &RevisionContract,
    output: &str,
) -> Result<RevisionHandoff> {
    validate_revision_handoff_with_evidence(contract, output, None)
}

/// Validate a revision handoff against the change evidence captured for this
/// revision. This check only confirms the handoff is structurally sound and
/// each claim is tied to real changed files; Orc runs deterministic validation
/// after the revision and before semantic Review.
pub fn validate_revision_handoff_with_evidence(
    contract: &RevisionContract,
    output: &str,
    changes: Option<&crate::git::WorktreeChanges>,
) -> Result<RevisionHandoff> {
    let active = contract.active_blocker_records();
    let active_count = active.len();
    // Preserve the legacy one-shot revision path when the authoritative ledger
    // contains no active blocker work. There is no claim to validate in that case.
    if active_count == 0 {
        return Ok(RevisionHandoff {
            completion: None,
            claims: Vec::new(),
        });
    }
    let mut handoff_value: serde_json::Value = serde_json::from_str(output)
        .context("revision worker did not return a structured handoff")?;
    if let Some(object) = handoff_value.as_object_mut() {
        object.remove("worker_protocol");
    }
    let handoff: RevisionHandoff = serde_json::from_value(handoff_value)
        .context("revision worker did not return a structured handoff")?;
    let required: std::collections::BTreeSet<_> =
        active.iter().map(|b| b.blocker_id.as_str()).collect();
    let mut seen = std::collections::BTreeSet::new();
    for claim in &handoff.claims {
        if !matches!(claim.status.as_str(), "addressed" | "unresolved") {
            bail!(
                "revision handoff claim '{}' has invalid status",
                claim.blocker_id
            );
        }
        active
            .iter()
            .find(|b| b.blocker_id == claim.blocker_id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocker ID '{}'", claim.blocker_id))?;
        if claim.implementation_summary.trim().is_empty()
            || is_vacuous_text(&claim.implementation_summary)
        {
            bail!(
                "revision handoff claim '{}' is missing implementation evidence",
                claim.blocker_id
            );
        }
        if claim.status == "addressed" && claim.changed_files.is_empty() {
            bail!(
                "revision handoff claim '{}' is vacuous: addressed blockers require changed files",
                claim.blocker_id
            );
        }
        if claim
            .changed_files
            .iter()
            .any(|path| path.trim().is_empty() || path.trim() == "[]")
        {
            bail!(
                "revision handoff claim '{}' contains placeholder changed files",
                claim.blocker_id
            );
        }
        let changes = changes.context("active revision claims require current change evidence")?;
        let mut actual_paths: std::collections::BTreeSet<&str> = changes
            .files
            .iter()
            .flat_map(|file| {
                if file.status.starts_with('R') || file.status.starts_with('C') {
                    file.path
                        .split_once(" -> ")
                        .into_iter()
                        .flat_map(|(a, b)| [a, b])
                        .collect::<Vec<_>>()
                } else {
                    vec![file.path.as_str()]
                }
            })
            .collect();
        for line in changes.diff.lines() {
            if let Some(path) = line
                .strip_prefix("--- a/")
                .or_else(|| line.strip_prefix("+++ b/"))
            {
                actual_paths.insert(path);
            }
        }
        if let Some(path) = claim
            .changed_files
            .iter()
            .find(|path| !actual_paths.contains(path.as_str()))
        {
            bail!(
                "revision handoff claim '{}' contains a file not changed in this revision: '{}' (current paths: {})",
                claim.blocker_id,
                path,
                actual_paths.iter().copied().collect::<Vec<_>>().join(", ")
            );
        }
        if !required.contains(claim.blocker_id.as_str()) {
            bail!(
                "revision handoff contains unknown blocker ID '{}'",
                claim.blocker_id
            );
        }
        if !seen.insert(claim.blocker_id.as_str()) {
            bail!(
                "revision handoff contains duplicate blocker ID '{}'",
                claim.blocker_id
            );
        }
    }
    if seen != required {
        bail!("revision handoff is missing one or more active blocker claims");
    }
    Ok(handoff)
}

fn is_vacuous_text(value: &str) -> bool {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    normalized.is_empty()
        || [
            "done",
            "fixed",
            "implemented",
            "tests pass",
            "n/a",
            "none",
            "todo",
            "tbd",
            "not tested",
        ]
        .contains(&normalized.as_str())
}

pub fn build_revision_contract_from_db(
    db: &crate::storage::Database,
    task_id: &str,
    reviews: &[crate::review::PriorReview],
    source_review_id: i64,
) -> Result<RevisionContract> {
    let source = reviews
        .iter()
        .find(|r| r.run_id == source_review_id)
        .context("source review not found")?;
    let source_ids: std::collections::BTreeSet<_> = source
        .blockers
        .iter()
        .map(|b| b.blocker_id.as_str())
        .collect();
    let mut contract = build_revision_contract_for_source_ids(db, task_id, &source_ids)?;
    // Legacy reviews without a persisted structured ledger retain compatibility.
    if ledger_is_empty_for_source(db, task_id, &source_ids)? {
        contract.unresolved = source
            .blockers
            .iter()
            .filter(|blocker| blocker.status != "resolved")
            .cloned()
            .collect();
    }
    let task = db
        .get_task(task_id)?
        .context("revision contract task not found")?;
    let task_contract = db
        .get_task_contract(task_id)?
        .unwrap_or_else(|| crate::task::TaskContract::defaults(&task.objective));
    contract.original_task_requirements = RevisionTaskRequirements {
        acceptance_criteria: task_contract.acceptance_criteria,
        required_tests: task_contract.required_tests,
        expected_changes: task.expected_changes.clone(),
        unchanged: task_contract.unchanged,
        validation: task_contract.validation,
    };
    contract.active_blockers = contract
        .unresolved
        .iter()
        .chain(contract.regressions.iter())
        .cloned()
        .collect();
    contract.resolved_blockers = contract.regression_constraints.clone();
    contract.reviewer_revision_feedback = source
        .revision_feedback
        .iter()
        .filter(|feedback| !feedback.trim().is_empty())
        .cloned()
        .collect();
    for run in db.list_agent_runs_for_task(task_id)? {
        if let Some((_, evidence)) = db.load_worker_protocol(run.id)?
            && evidence.is_some()
        {
            contract.current_persisted_execution_evidence = evidence;
            break;
        }
    }
    contract.validation_failures = source
        .validation_evidence
        .as_deref()
        .and_then(|value| serde_json::from_str::<crate::validation::ValidationReport>(value).ok())
        .map(|report| {
            report
                .steps
                .iter()
                .filter(|step| !step.passed)
                .map(|step| format!("{}: {}", step.command, step.output()))
                .collect()
        })
        .unwrap_or_default();
    Ok(contract)
}

fn build_revision_contract_for_source_ids(
    db: &crate::storage::Database,
    task_id: &str,
    source_ids: &std::collections::BTreeSet<&str>,
) -> Result<RevisionContract> {
    let ledger = db.review_blocker_ledger(task_id)?;
    let mut unresolved = Vec::new();
    let mut regressions = Vec::new();
    let mut constraints = Vec::new();
    for record in ledger {
        if source_ids.contains(record.blocker_id.as_str()) {
            match record.status.as_str() {
                "resolved" => constraints.push(record),
                "regression" => regressions.push(record),
                _ => unresolved.push(record),
            }
        }
    }
    Ok(RevisionContract {
        unresolved,
        regressions,
        regression_constraints: constraints,
        ..RevisionContract::default()
    })
}

fn ledger_is_empty_for_source(
    db: &crate::storage::Database,
    task_id: &str,
    ids: &std::collections::BTreeSet<&str>,
) -> Result<bool> {
    Ok(ids.is_empty()
        || db
            .review_blocker_ledger(task_id)?
            .iter()
            .all(|b| !ids.contains(b.blocker_id.as_str())))
}

pub fn build_revision_contract(
    reviews: &[crate::review::PriorReview],
    source_review_id: i64,
) -> RevisionContract {
    let source = reviews
        .iter()
        .find(|review| review.run_id == source_review_id);
    let source_ids: std::collections::BTreeSet<String> = source
        .into_iter()
        .flat_map(|review| review.blockers.iter().map(|b| b.blocker_id.clone()))
        .collect();
    let mut ledger = std::collections::BTreeMap::new();
    for review in reviews {
        for blocker in &review.blockers {
            if source_ids.contains(&blocker.blocker_id) {
                ledger.insert(blocker.blocker_id.clone(), blocker.clone());
            }
        }
        if review.verdict == "PASS" && review.run_id > source_review_id {
            for record in ledger.values_mut() {
                record.status = "resolved".into();
            }
        }
    }
    if ledger.is_empty()
        && let Some(review) = source
    {
        for finding in &review.blocking_findings {
            let record = crate::storage::db::ReviewBlockerRecord {
                task_id: String::new(),
                blocker_id: blocker_id(finding),
                run_id: review.run_id,
                requirement_ref: String::new(),
                evidence: finding.clone(),
                severity: review
                    .severity
                    .clone()
                    .unwrap_or_else(|| "unspecified".into()),
                acceptance_condition: review.revision_feedback.clone().unwrap_or_else(|| {
                    "Address the finding and provide evidence it is resolved.".into()
                }),
                status: "unresolved".into(),
                finding: finding.clone(),
                first_seen: String::new(),
                last_seen: String::new(),
                blocker_key: finding.clone(),
            };
            ledger.insert(record.blocker_id.clone(), record);
        }
    }
    let mut unresolved = Vec::new();
    let mut regressions = Vec::new();
    let mut regression_constraints = Vec::new();
    for record in ledger.into_values() {
        match record.status.as_str() {
            "resolved" => regression_constraints.push(record),
            "regression" => regressions.push(record),
            _ => unresolved.push(record),
        }
    }
    let active_blockers = unresolved
        .iter()
        .chain(regressions.iter())
        .cloned()
        .collect();
    let resolved_blockers = regression_constraints.clone();
    RevisionContract {
        active_blockers,
        resolved_blockers,
        unresolved,
        regressions,
        regression_constraints,
        ..RevisionContract::default()
    }
}

pub fn format_revision_contract(contract: &RevisionContract) -> String {
    let mut out = String::from("## Revision contract\n\n");
    out.push_str("### Original task requirements (preserve and satisfy)\n");
    out.push_str(&format!(
        "- Acceptance criteria: {:?}\n- Required tests: {:?}\n- Expected changes: {:?}\n- Unchanged constraints: {:?}\n- Required validation: {:?}\n",
        contract.original_task_requirements.acceptance_criteria,
        contract.original_task_requirements.required_tests,
        contract.original_task_requirements.expected_changes,
        contract.original_task_requirements.unchanged,
        contract.original_task_requirements.validation,
    ));
    out.push_str("### Unresolved blockers (implement and prove each)\n");
    let active = contract.active_blocker_records();
    if active.is_empty() {
        out.push_str("- None recorded; verify the supplied review feedback.\n");
    }
    for blocker in active {
        out.push_str(&format!(
            "- {} | requirement: {} | acceptance: {} | finding: {}\n  Evidence required: {}\n",
            blocker.blocker_id,
            blocker.requirement_ref,
            blocker.acceptance_condition,
            blocker.finding,
            blocker.evidence
        ));
    }
    out.push_str("### Resolved blockers (regression constraints; do not reimplement)\n");
    let resolved = if contract.resolved_blockers.is_empty() {
        &contract.regression_constraints
    } else {
        &contract.resolved_blockers
    };
    if resolved.is_empty() {
        out.push_str("- None recorded.\n");
    }
    for blocker in resolved {
        out.push_str(&format!("- {} | acceptance: {} | preserve the resolved behavior unless current evidence proves regression\n", blocker.blocker_id, blocker.acceptance_condition));
    }
    out.push_str("### Regressions (implement and prove each)\n");
    for blocker in &contract.regressions {
        out.push_str(&format!(
            "- {} | acceptance: {} | finding: {}\n",
            blocker.blocker_id, blocker.acceptance_condition, blocker.finding
        ));
    }
    out.push_str("### Reviewer revision feedback (context, not a replacement for requirements)\n");
    if contract.reviewer_revision_feedback.is_empty() {
        out.push_str("- None recorded.\n");
    }
    for feedback in &contract.reviewer_revision_feedback {
        out.push_str(&format!("- {feedback}\n"));
    }
    out.push_str("### Current persisted execution evidence\n");
    if let Some(evidence) = &contract.current_persisted_execution_evidence {
        out.push_str(&format!("- {evidence:?}\n"));
    } else {
        out.push_str("- None recorded.\n");
    }
    out.push_str("### Validation failures\n");
    if contract.validation_failures.is_empty() {
        out.push_str("- None recorded.\n");
    }
    for failure in &contract.validation_failures {
        out.push_str(&format!("- {failure}\n"));
    }
    out.push_str("\n### Required handoff\nReturn JSON {\"completion\":{\"step_results\":[{\"step_id\":\"...\",\"operations_performed\":[\"modify\"],\"affected_files\":[],\"observed\":[],\"verification_passed\":[]}],\"summary\":\"...\"},\"claims\":[{\"blocker_id\":\"...\",\"status\":\"addressed|unresolved\",\"implementation_summary\":\"...\",\"changed_files\":[],\"unresolved_risk\":null}]} with exactly one claim for every active blocker ID. `completion` describes the work performed; Orc derives changed files from the worktree and owns deterministic validation, acceptance, and reviewer-style verification. Resolved constraints require no claim. `completion` is the same checkpoint evidence contract used by initial implementation; claims add only revision blocker disposition. Do not include validation evidence in a claim \u{2014} Orc owns validation. Keep changes focused.");
    out
}

impl ReviewResult {
    fn validate_structured_blockers(&self) -> Result<()> {
        for blocker in &self.blockers {
            if blocker.blocker_key.trim().is_empty() {
                bail!("review blockers require a non-empty blocker_key")
            }
            if !matches!(
                blocker.status.as_str(),
                "new" | "unresolved" | "resolved" | "regression"
            ) {
                bail!("review blocker has invalid status '{}'", blocker.status)
            }
        }
        Ok(())
    }
}

pub fn blocker_id(finding: &str) -> String {
    let normalized = finding
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut hash: u64 = 14695981039346656037;
    for byte in normalized.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(1099511628211);
    }
    format!("BLK-{hash:016x}")
}

pub fn structured_blocker_id(
    requirement_ref: &str,
    acceptance_condition: &str,
    finding: &str,
) -> String {
    let key = [requirement_ref, acceptance_condition]
        .iter()
        .map(|s| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("|");
    if key.is_empty() {
        blocker_id(finding)
    } else {
        blocker_id(&key)
    }
}

fn normalize_blockers(result: &mut ReviewResult) {
    if result.blockers.is_empty() {
        result.blockers = result
            .blocking_findings
            .iter()
            .map(|finding| ReviewBlocker {
                id: structured_blocker_id(
                    "",
                    &result.revision_feedback.clone().unwrap_or_default(),
                    finding,
                ),
                prior_blocker_id: None,
                blocker_key: structured_blocker_key(
                    "",
                    &result.revision_feedback.clone().unwrap_or_default(),
                    finding,
                ),
                requirement_ref: String::new(),
                evidence: finding.clone(),
                severity: result
                    .severity
                    .clone()
                    .unwrap_or_else(|| "unspecified".into()),
                acceptance_condition: result.revision_feedback.clone().unwrap_or_else(|| {
                    "Address the finding and provide evidence it is resolved.".into()
                }),
                status: "new".into(),
                finding: finding.clone(),
            })
            .collect();
    }
    result.blocking_findings = result
        .blockers
        .iter()
        .filter(|b| b.status != "resolved")
        .map(|b| b.finding.clone())
        .collect();
}

fn structured_blocker_key(
    requirement_ref: &str,
    acceptance_condition: &str,
    finding: &str,
) -> String {
    let key = [requirement_ref, acceptance_condition, finding]
        .iter()
        .map(|value| {
            value
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("|");
    if key.is_empty() {
        "legacy-blocker".into()
    } else {
        key
    }
}

#[cfg(test)]
fn review_resolution_ledger(reviews: &[crate::review::PriorReview]) -> String {
    if reviews.is_empty() {
        return "No prior task reviews.".into();
    }
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for review in reviews {
        for blocker in &review.blockers {
            let entry = format!(
                "{} | key={} | requirement={} | acceptance={} | status={}",
                blocker.blocker_id,
                blocker.blocker_key,
                blocker.requirement_ref,
                blocker.acceptance_condition,
                blocker.status
            );
            if blocker.status == "resolved" {
                resolved.push(entry);
            } else {
                unresolved.push(entry);
            }
        }
    }
    // Compatibility for historical review rows created before the structured ledger existed.
    if resolved.is_empty() && unresolved.is_empty() {
        let mut legacy_unresolved = Vec::new();
        let mut legacy_resolved = Vec::new();
        for review in reviews {
            if review.verdict.eq_ignore_ascii_case("pass") {
                legacy_resolved.append(&mut legacy_unresolved);
                continue;
            }
            legacy_unresolved.extend(review.blocking_findings.iter().cloned());
        }
        resolved = legacy_resolved;
        unresolved = legacy_unresolved;
    }
    let mut ledger = String::new();
    if !resolved.is_empty() {
        ledger.push_str(
            "RESOLVED prior blockers (do not reintroduce without current regression evidence):\n",
        );
        for finding in resolved {
            ledger.push_str("- ");
            ledger.push_str(&finding);
            ledger.push('\n');
        }
    }
    if !unresolved.is_empty() {
        ledger.push_str("UNRESOLVED prior blockers (recheck against current evidence):\n");
        for finding in unresolved {
            ledger.push_str("- ");
            ledger.push_str(&finding);
            ledger.push('\n');
        }
    }
    ledger
}

const TASK_REVIEW_INSTRUCTIONS: &str = "Perform an acceptance-first, task-scoped contract review using only the supplied task contract, submitted diff/change evidence, blocker ledger/revision history, and Orc-produced structured validation evidence. Do not execute shell commands, tests, cargo/npm/formatting commands, validation, or repository discovery; Orc already selected and ran task-specific validation. Treat its validation results as authoritative for command outcomes. On a revision, assess every unresolved blocker against the supplied current evidence before considering a broad review. Check each resolved blocker for regression; equivalent or reworded findings refer to the same concern and remain resolved. Reopen a resolved concern only when supplied current evidence demonstrates a genuine regression. Clearly distinguish RESOLVED from UNRESOLVED prior blockers. Do not restate equivalent findings. Reject vacuous or placeholder tests, assertions, changed-file lists, and validation claims. A blocker must identify an explicit requirement, concrete supplied evidence, and why acceptance is prevented; only unmet requirements, incorrect required workflow, material regressions, safety/data-integrity failures, or failed/materially absent structured validation can block. Keep blocking findings to at most 5. PASS requires no blocking findings; REVISE requires focused in-scope changes; REJECT is only for fundamental contradiction or unsafe implementation.";

fn start_run(db: &Database, action: AgentAction, resolved: &ResolvedAction) -> Result<i64> {
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    Ok(db.create_project_action_run(
        project_id,
        None,
        action.as_str(),
        &resolved.agent,
        AgentRunExecution {
            class: action.as_str(),
            model: resolved.model.as_deref(),
            effort: resolved.reasoning_effort,
            source: &resolved.resolution_record.source,
        },
    )?)
}

fn announce_run(backend: &dyn ActionBackend, run: i64, resolved: &ResolvedAction) {
    if resolved.action == AgentAction::Review {
        backend.observe(&format!(
            "Starting reviewer             {} / {}",
            resolved.agent,
            resolved.model.as_deref().unwrap_or("default"),
        ));
        return;
    }
    backend.observe(&format!(
        "Automated {} run {}: agent={} model={} reasoning_effort={}",
        resolved.action.as_str(),
        run,
        resolved.agent,
        resolved.model.as_deref().unwrap_or("default"),
        resolved
            .reasoning_effort
            .map(|value| value.as_str())
            .unwrap_or("default")
    ));
}

fn invoke_action(
    db: &Database,
    run: i64,
    backend: &dyn ActionBackend,
    agent: &AgentDefinition,
    resolved: &ResolvedAction,
    prompt: &str,
) -> Result<ActionExecution> {
    announce_run(backend, run, resolved);
    let invocation = db.start_provider_invocation_with_resolution(
        run,
        resolved.action.as_str(),
        1,
        &resolved.resolution_record,
    )?;
    let phase = if resolved.action == AgentAction::Review {
        "Reviewing implementation      ..."
    } else {
        "provider starting"
    };
    db.update_agent_run_phase(run, phase)?;
    backend.observe(phase);
    let progress = |activity: &str| {
        if let Err(error) = db.update_agent_run_phase(run, activity) {
            backend.observe(&format!(
                "warning: failed to persist action progress: {error}"
            ));
        }
        backend.observe(activity);
    };
    let action_schema = schema(resolved.action);
    let execution = backend.invoke_with_progress(
        agent,
        resolved.action,
        prompt,
        resolved.model.as_deref(),
        resolved.reasoning_effort,
        ActionProgress {
            schema: &action_schema,
            callback: &progress,
        },
    );
    db.finish_provider_invocation(
        invocation,
        if execution.is_ok() {
            "completed"
        } else {
            "failed"
        },
        execution.as_ref().ok().and_then(|value| value.token_usage),
    )?;
    execution
}

fn fail_run(
    db: &Database,
    run: i64,
    error: &anyhow::Error,
    usage: Option<TokenUsage>,
) -> Result<()> {
    db.update_agent_run_failure(run, None, &error.to_string(), usage)?;
    Ok(())
}

fn parse_structured<T: for<'de> Deserialize<'de>>(output: &str, label: &str) -> Result<T> {
    if output.trim().is_empty() {
        bail!("{label} returned empty structured output")
    }
    serde_json::from_str(output)
        .with_context(|| format!("{label} returned malformed structured output"))
}

/// Build the bounded, typed Planner contract. Full reports and operational
/// histories remain in SQLite and are not replayed to the provider.
pub fn build_planner_packet(
    db: &Database,
    request: &PlanningRequest,
) -> Result<crate::execution_packet::PlannerPacket> {
    let mut decisions = db.pending_lead_decision_context()?;
    decisions.sort_by_key(|decision| decision.id);
    let omitted_decisions = decisions.len().saturating_sub(4);
    decisions.truncate(4);
    let decisions = decisions
        .into_iter()
        .map(|decision| crate::execution_packet::PlannerDecisionContext {
            id: decision.id,
            kind: decision.kind,
            source_request: crate::execution_packet::BoundedText::new(
                &decision.source_request,
                8_000,
            ),
            summary: crate::execution_packet::BoundedText::new(&decision.summary, 4_000),
            details: crate::execution_packet::BoundedText::new(&decision.details, 12_000),
            resolution: decision
                .resolution
                .as_deref()
                .map(|value| crate::execution_packet::BoundedText::new(value, 4_000)),
        })
        .collect::<Vec<_>>();
    let mut state = request.current_state.clone();
    let mut omitted_state = 0;
    if let Some(state) = &mut state {
        for tasks in [
            &mut state.ready_tasks,
            &mut state.active_tasks,
            &mut state.review_tasks,
            &mut state.blocked_tasks,
        ] {
            omitted_state += tasks.len().saturating_sub(32);
            tasks.truncate(32);
        }
        state.usable_agents.truncate(32);
        state.busy_agents.truncate(32);
    }
    let (discovery_snapshot, omitted_discovery) = request
        .discovery_snapshot
        .clone()
        .map(bound_discovery)
        .map_or((None, 0), |(snapshot, omitted)| (Some(snapshot), omitted));
    let engineering_contract = crate::execution_packet::BoundedText::new(
        &request.engineering_contract,
        crate::execution_packet::MAX_ENGINEERING_BYTES,
    );
    let objective = crate::execution_packet::BoundedText::new(&request.objective, 8_000);
    let mut truncations = Vec::new();
    if engineering_contract.truncated() {
        truncations.push(crate::execution_packet::Truncation {
            field: "engineering_contract".into(),
            omitted_items: 0,
            omitted_bytes: engineering_contract.omitted_bytes,
        });
    }
    if objective.truncated() {
        truncations.push(crate::execution_packet::Truncation {
            field: "objective".into(),
            omitted_items: 0,
            omitted_bytes: objective.omitted_bytes,
        });
    }
    for (field, omitted) in [
        ("source_lead_decision", omitted_decisions),
        ("current_state.tasks", omitted_state),
        ("discovery_snapshot", omitted_discovery),
    ] {
        if omitted != 0 {
            truncations.push(crate::execution_packet::Truncation {
                field: field.into(),
                omitted_items: omitted,
                omitted_bytes: 0,
            });
        }
    }
    for (field, values) in [
        ("constraints", &request.constraints),
        ("non_goals", &request.non_goals),
        ("deliverables", &request.deliverables),
        ("definition_of_done", &request.definition_of_done),
        ("role_boundaries", &request.role_boundaries),
        ("planning_constraints", &request.planning_constraints),
        ("approval_requirements", &request.approval_requirements),
    ] {
        let omitted = values
            .len()
            .saturating_sub(crate::execution_packet::MAX_LIST_ITEMS);
        if omitted != 0 {
            truncations.push(crate::execution_packet::Truncation {
                field: field.into(),
                omitted_items: omitted,
                omitted_bytes: 0,
            });
        }
    }
    let list = |values: &[String]| {
        values
            .iter()
            .take(crate::execution_packet::MAX_LIST_ITEMS)
            .cloned()
            .collect()
    };
    Ok(crate::execution_packet::PlannerPacket {
        metadata: crate::execution_packet::PacketMetadata {
            packet_type: "planner".into(),
            truncations,
            ..Default::default()
        },
        protocol_version: request.protocol_version,
        kind: request.kind.clone(),
        objective,
        project: request.project.clone(),
        engineering_contract,
        constraints: list(&request.constraints),
        non_goals: list(&request.non_goals),
        deliverables: list(&request.deliverables),
        definition_of_done: list(&request.definition_of_done),
        role_boundaries: list(&request.role_boundaries),
        planning_constraints: list(&request.planning_constraints),
        approval_requirements: list(&request.approval_requirements),
        current_state: state,
        discovery_snapshot,
        source_lead_decision: decisions,
        response_schema: request.response_schema.clone(),
    })
}

pub fn planner_packet(db: &Database, request: &PlanningRequest) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(build_planner_packet(db, request)?)?)
}

/// Build the typed semantic Review packet. Lifecycle freshness is checked
/// before this collector is reached; the packet additionally refuses failed
/// evidence and contains no command output or repository access request.
pub fn build_review_packet(
    db: &Database,
    summary: &ReviewSummary,
) -> Result<crate::execution_packet::ReviewPacket> {
    let contract = db
        .get_task_contract(&summary.task.id)?
        .unwrap_or_else(|| crate::task::TaskContract::defaults(&summary.task.objective));
    let validation = summary
        .validation_evidence
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("persisted validation evidence is invalid")?
        .unwrap_or(crate::validation::ValidationReport { steps: Vec::new() });
    crate::execution_packet::ReviewPacket::build(
        &summary.task,
        &contract,
        summary.run.as_ref().map(|run| run.id),
        &summary.changes,
        &validation,
        &db.review_blocker_ledger(&summary.task.id)?,
    )
}

/// Compatibility inspection surface for callers that consume JSON values.
pub fn review_packet(db: &Database, summary: &ReviewSummary) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(build_review_packet(db, summary)?)?)
}

fn review_agent_without_command_execution(agent: &AgentDefinition) -> AgentDefinition {
    let mut review_agent = agent.clone();
    review_agent.capabilities.retain(|capability| {
        !matches!(
            crate::registry::AgentCapability::parse(capability),
            crate::registry::AgentCapability::CommandExecution
        )
    });
    review_agent
}

pub fn build_lead_packet(
    context: &LeadContext,
    message: &str,
) -> crate::execution_packet::LeadPacket {
    let mut active_tasks = context
        .tasks
        .iter()
        .filter(|task| !task.status.is_terminal())
        .cloned()
        .collect::<Vec<_>>();
    active_tasks.sort_by(|left, right| left.id.cmp(&right.id));
    let omitted_tasks = active_tasks.len().saturating_sub(50);
    active_tasks.truncate(50);
    let active_ids = active_tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let dependencies = context
        .dependencies
        .iter()
        .filter(|(task, _)| active_ids.contains(task.as_str()))
        .map(|(task, dependencies)| (task.clone(), dependencies.clone()))
        .collect();
    let mut pending_approvals = context
        .approvals
        .iter()
        .filter(|item| !item.resolved)
        .cloned()
        .collect::<Vec<_>>();
    let omitted_approvals = pending_approvals.len().saturating_sub(20);
    pending_approvals.truncate(20);
    let omitted_facts = context.facts.len().saturating_sub(50);
    let mut planning_state = context.state.clone();
    let mut omitted_state = 0;
    for tasks in [
        &mut planning_state.ready_tasks,
        &mut planning_state.active_tasks,
        &mut planning_state.review_tasks,
        &mut planning_state.blocked_tasks,
    ] {
        omitted_state += tasks.len().saturating_sub(32);
        tasks.truncate(32);
    }
    planning_state.usable_agents.truncate(32);
    planning_state.busy_agents.truncate(32);
    let (discovery, omitted_discovery) = context
        .discovery
        .clone()
        .map(bound_discovery)
        .map_or((None, 0), |(snapshot, omitted)| (Some(snapshot), omitted));
    let mut truncations = Vec::new();
    for (field, omitted) in [
        ("active_tasks", omitted_tasks),
        ("discovery", omitted_discovery),
        ("pending_approvals", omitted_approvals),
        ("facts", omitted_facts),
        ("planning_state.tasks", omitted_state),
    ] {
        if omitted != 0 {
            truncations.push(crate::execution_packet::Truncation {
                field: field.into(),
                omitted_items: omitted,
                omitted_bytes: 0,
            });
        }
    }
    let request = crate::execution_packet::BoundedText::new(message, 48_000);
    let engineering_contract = crate::execution_packet::BoundedText::new(
        &context.engineering_contract,
        crate::execution_packet::MAX_ENGINEERING_BYTES,
    );
    let architecture = context
        .architecture
        .as_deref()
        .map(|value| crate::execution_packet::BoundedText::new(value, 16_000));
    let facts = context
        .facts
        .iter()
        .take(50)
        .map(|(key, value)| {
            (
                key.clone(),
                crate::execution_packet::BoundedText::new(value, 2_000),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let omitted_fact_bytes = facts.values().map(|value| value.omitted_bytes).sum();
    for (field, value) in [
        ("request", Some(&request)),
        ("engineering_contract", Some(&engineering_contract)),
        ("architecture", architecture.as_ref()),
    ] {
        if let Some(value) = value
            && value.truncated()
        {
            truncations.push(crate::execution_packet::Truncation {
                field: field.into(),
                omitted_items: 0,
                omitted_bytes: value.omitted_bytes,
            });
        }
    }
    if omitted_fact_bytes != 0 {
        truncations.push(crate::execution_packet::Truncation {
            field: "facts".into(),
            omitted_items: 0,
            omitted_bytes: omitted_fact_bytes,
        });
    }
    crate::execution_packet::LeadPacket {
        metadata: crate::execution_packet::PacketMetadata {
            packet_type: "lead".into(),
            truncations,
            ..Default::default()
        },
        request,
        project_id: context.project_id,
        project_name: context.project_name.clone(),
        discovery,
        engineering_contract,
        architecture,
        facts: facts
            .into_iter()
            .map(|(key, value)| (key, value.text))
            .collect(),
        planning_state,
        active_tasks,
        dependencies,
        pending_approvals,
    }
}

fn bound_discovery(
    mut snapshot: crate::discovery::ProjectDiscoverySnapshot,
) -> (crate::discovery::ProjectDiscoverySnapshot, usize) {
    let mut omitted = 0;
    for values in [
        &mut snapshot.important_files,
        &mut snapshot.manifests,
        &mut snapshot.test_locations,
        &mut snapshot.architecture_boundaries,
        &mut snapshot.unknowns_and_risks,
        &mut snapshot.validation_commands,
        &mut snapshot.technology_stack,
        &mut snapshot.architecture.entry_points,
        &mut snapshot.architecture.source_directories,
    ] {
        omitted += values.len().saturating_sub(64);
        values.truncate(64);
    }
    omitted += snapshot.repository.changed_files.len().saturating_sub(64);
    snapshot.repository.changed_files.truncate(64);
    if let Some(contract) = snapshot.project.engineering_contract.take() {
        // The authoritative bounded engineering contract is already a stable
        // top-level packet field; do not repeat it in volatile discovery.
        omitted += usize::from(!contract.is_empty());
    }
    if let Some(description) = &mut snapshot.project.description {
        let bounded = crate::execution_packet::BoundedText::new(description, 4_000);
        omitted += usize::from(bounded.truncated());
        *description = bounded.text;
    }
    for tasks in [
        &mut snapshot.task_state.ready_tasks,
        &mut snapshot.task_state.active_tasks,
        &mut snapshot.task_state.review_tasks,
        &mut snapshot.task_state.blocked_tasks,
    ] {
        omitted += tasks.len().saturating_sub(32);
        tasks.truncate(32);
    }
    snapshot.task_state.usable_agents.truncate(32);
    snapshot.task_state.busy_agents.truncate(32);
    (snapshot, omitted)
}

/// Task review consumes fresh deterministic validation evidence created by
/// Dispatch or Revise, then evaluates only semantic task-contract concerns.
pub fn run_review(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
    _repo_path: &Path,
    _validation_runner: &dyn ValidationRunner,
) -> Result<(i64, ReviewResult)> {
    require_fresh_passing_validation(db, summary)?;
    run_review_mode(db, summary, overrides, backend, false, None)
}

fn require_fresh_passing_validation(db: &Database, summary: &ReviewSummary) -> Result<()> {
    let Some(run) = summary.run.as_ref() else {
        return Ok(());
    };
    let Some(worktree) = summary.worktree_path.as_ref() else {
        return Ok(());
    };
    if worktree.is_empty() {
        return Ok(());
    }
    let report: crate::validation::ValidationReport = db
        .latest_validation_result_for_run(run.id)?
        .context("review requires current passing deterministic validation evidence")
        .and_then(|value| {
            serde_json::from_str(&value).context("persisted validation evidence is invalid")
        })?;
    if !report.is_success() {
        bail!(
            "review requires current passing deterministic validation evidence; return to implementation-stage repair"
        );
    }
    let selection: serde_json::Value = db
        .latest_validation_selection_for_run(run.id)?
        .context("review requires validation freshness evidence")
        .and_then(|value| {
            serde_json::from_str(&value).context("persisted validation selection is invalid")
        })?;
    let current = revision_worktree_fingerprint(&summary.changes);
    if selection["worktree_fingerprint"].as_str() != Some(current.as_str()) {
        bail!(
            "review validation evidence is stale for the current worktree; return to implementation-stage validation"
        );
    }
    Ok(())
}

/// Project-wide audit. Unlike [`run_review`], this never gates task
/// acceptance (it only ever persists as a completed action run, never a
/// PASS/REVISE/REJECT task verdict via `commit_task_review_result`), so it
/// has no task-specific validation to own.
pub fn run_project_review(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
) -> Result<(i64, ReviewResult)> {
    run_review_mode(db, summary, overrides, backend, true, None)
}

fn run_review_mode(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
    project_review: bool,
    _validation: Option<(&Path, &dyn ValidationRunner)>,
) -> Result<(i64, ReviewResult)> {
    if !project_review && summary.task.status.is_terminal() {
        bail!(
            "task {} cannot be reviewed from terminal status {}",
            summary.task.id,
            summary.task.status
        );
    }
    let (agent, resolved) = resolve_action_with_transport(
        db,
        AgentAction::Review,
        overrides,
        backend.transport_eligibility(),
        backend.quota_refresher(),
    )?;
    let review_agent = review_agent_without_command_execution(&agent);
    let run = db.create_project_action_run(
        db.get_project_id()?.context("no project found in DB")?,
        (!project_review).then_some(summary.task.id.as_str()),
        AgentAction::Review.as_str(),
        &resolved.agent,
        AgentRunExecution {
            class: AgentAction::Review.as_str(),
            model: resolved.model.as_deref(),
            effort: resolved.reasoning_effort,
            source: &resolved.resolution_record.source,
        },
    )?;
    let _run_finalizer = db.run_finalizer(run);
    if !project_review {
        backend.observe("Preparing review packet       OK");
    }
    let instructions = if project_review {
        "Perform a project-wide audit. Inspect broader architecture, latent defects, consistency, technical debt, missing tests, and adjacent concerns without task-scope restrictions. Classify findings in blocking_findings or non_blocking_findings for this project audit."
    } else {
        TASK_REVIEW_INSTRUCTIONS
    };
    let result_contract = "Return only JSON matching {\"verdict\":string,\"findings\":[string],\"blocking_findings\":[string],\"non_blocking_findings\":[string],\"severity\":string|null,\"revision_feedback\":string|null,\"blockers\":[{\"id\":string,\"prior_blocker_id\":string|null,\"blocker_key\":string,\"requirement_ref\":string,\"evidence\":string,\"severity\":string,\"acceptance_condition\":string,\"status\":\"new|unresolved|resolved|regression\",\"finding\":string}]}. blocker_key is readable context, not identity. Copy an existing blocker_id verbatim into prior_blocker_id for the same concern; use null only for a genuinely new blocker. Review semantic task-contract concerns only. Do not accept or merge the task.";
    let prompt = if project_review {
        let contract = db
            .get_task_contract(&summary.task.id)?
            .unwrap_or_else(|| crate::task::TaskContract::defaults(&summary.task.objective));
        crate::execution_packet::render_packet(
            &format!("{instructions} {result_contract}"),
            &crate::execution_packet::ProjectReviewPacket::build(
                &summary.task,
                &contract,
                &summary.changes,
            ),
        )?
    } else {
        crate::execution_packet::render_packet(
            &format!("{instructions} {result_contract}"),
            &build_review_packet(db, summary)?,
        )?
    };
    let execution = invoke_action(db, run, backend, &review_agent, &resolved, &prompt);
    match execution {
        Ok(execution) => {
            let parsed = parse_structured::<ReviewResult>(&execution.output, "reviewer").and_then(
                |result| {
                    if result.verdict.trim().is_empty() {
                        bail!("review verdict must not be empty")
                    }
                    let mut result = result;
                    if !project_review {
                        result.blocking_findings.truncate(5);
                    }
                    if result.blocking_findings.is_empty()
                        && result.non_blocking_findings.is_empty()
                        && (result.verdict.eq_ignore_ascii_case("revise")
                            || result.verdict.eq_ignore_ascii_case("reject"))
                        && !result.findings.is_empty()
                    {
                        result.blocking_findings = result.findings.clone();
                    }
                    if !result.blocking_findings.is_empty()
                        && result.verdict.eq_ignore_ascii_case("pass")
                    {
                        result.verdict = "REVISE".into();
                    }
                    if !project_review
                        && result.blocking_findings.is_empty()
                        && (result.verdict.eq_ignore_ascii_case("revise")
                            || result.verdict.eq_ignore_ascii_case("reject"))
                    {
                        result.verdict = "PASS".into();
                    }
                    if result.verdict.eq_ignore_ascii_case("pass") {
                        result.blocking_findings.clear();
                    }
                    normalize_blockers(&mut result);
                    result.validate_structured_blockers()?;
                    let prior = db.review_blocker_ledger(&summary.task.id)?;
                    let mut referenced = std::collections::HashSet::new();
                    for blocker in &mut result.blockers {
                        if let Some(returned_id) = blocker.prior_blocker_id.as_deref() {
                            let old = if let Some(exact) =
                                prior.iter().find(|old| old.blocker_id == returned_id)
                            {
                                exact
                            } else {
                                let mut keyed = prior.iter().filter(|old| {
                                    old.blocker_key == blocker.blocker_key
                                });
                                let unique = keyed.next();
                                match (unique, keyed.next()) {
                                    (Some(old), None) => old,
                                    _ => bail!("prior_blocker_id '{returned_id}' does not belong to task '{}'", summary.task.id),
                                }
                            };
                            let canonical_id = old.blocker_id.clone();
                            if !referenced.insert(canonical_id.clone()) {
                                bail!("prior_blocker_id '{canonical_id}' is referenced by duplicate findings")
                            }
                            blocker.prior_blocker_id = Some(canonical_id.clone());
                            blocker.id = canonical_id.clone();
                            // A resolved blocker remains resolved unless the structured
                            // review explicitly labels the current evidence a regression.
                            // Merely referencing it (including an unresolved/reworded
                            // finding) must not reopen the canonical blocker.
                            blocker.status = match (old.status.as_str(), blocker.status.as_str()) {
                                ("resolved", "regression") => "regression",
                                ("resolved", "resolved" | "unresolved") => "resolved",
                                ("new" | "unresolved" | "regression", "resolved") => "resolved",
                                ("new" | "unresolved" | "regression", "new" | "unresolved") => "unresolved",
                                ("new" | "unresolved" | "regression", "regression") => "regression",
                                ("new" | "unresolved" | "regression", _) => {
                                    bail!(
                                        "invalid status transition for prior blocker '{}'",
                                        canonical_id
                                    )
                                }
                                (_, _) => bail!(
                                    "invalid persisted status '{}' for prior blocker '{}'",
                                    old.status, canonical_id
                                ),
                            }
                            .into();
                        } else {
                            blocker.id = if blocker.blocker_key.trim().is_empty() {
                                structured_blocker_id(
                                    &blocker.requirement_ref,
                                    &blocker.acceptance_condition,
                                    &blocker.finding,
                                )
                            } else {
                                blocker_id(&blocker.blocker_key)
                            };
                            blocker.status = "new".into();
                        }
                    }
                    if result.verdict.eq_ignore_ascii_case("pass") {
                        let resolved =
                            prior
                                .iter()
                                .filter(|old| old.status != "resolved")
                                .map(|old| ReviewBlocker {
                                    id: old.blocker_id.clone(),
                                    prior_blocker_id: Some(old.blocker_id.clone()),
                                    blocker_key: old.blocker_key.clone(),
                                    requirement_ref: old.requirement_ref.clone(),
                                    evidence: "No blocking finding in the current review.".into(),
                                    severity: old.severity.clone(),
                                    acceptance_condition: old.acceptance_condition.clone(),
                                    status: "resolved".into(),
                                    finding: old.finding.clone(),
                                });
                        result.blockers.extend(resolved);
                    }
                    Ok(result)
                },
            );
            match parsed {
                Ok(result) => {
                    if !project_review {
                        backend.observe(&format!(
                            "Reviewer finished             {}",
                            result.verdict.to_ascii_uppercase()
                        ));
                    }
                    let persisted_output = serde_json::to_string(&result)?;
                    if !project_review {
                        db.store_change_evidence(run, &summary.changes)?;
                        let contract = if result.verdict.eq_ignore_ascii_case("revise") {
                            let records = result.blockers.iter().map(|blocker| {
                                crate::storage::db::ReviewBlockerRecord {
                                    task_id: summary.task.id.clone(),
                                    blocker_id: blocker.id.clone(),
                                    run_id: run,
                                    blocker_key: blocker.blocker_key.clone(),
                                    requirement_ref: blocker.requirement_ref.clone(),
                                    evidence: blocker.evidence.clone(),
                                    severity: blocker.severity.clone(),
                                    acceptance_condition: blocker.acceptance_condition.clone(),
                                    status: blocker.status.clone(),
                                    finding: blocker.finding.clone(),
                                    first_seen: String::new(),
                                    last_seen: String::new(),
                                }
                            });
                            let mut contract = RevisionContract {
                                unresolved: Vec::new(),
                                regressions: Vec::new(),
                                regression_constraints: Vec::new(),
                                ..RevisionContract::default()
                            };
                            for record in records {
                                match record.status.as_str() {
                                    "resolved" => contract.regression_constraints.push(record),
                                    "regression" => contract.regressions.push(record),
                                    _ => contract.unresolved.push(record),
                                }
                            }
                            let task_contract =
                                db.get_task_contract(&summary.task.id)?.unwrap_or_else(|| {
                                    crate::task::TaskContract::defaults(&summary.task.objective)
                                });
                            contract.original_task_requirements = RevisionTaskRequirements {
                                acceptance_criteria: task_contract.acceptance_criteria,
                                required_tests: task_contract.required_tests,
                                expected_changes: summary.task.expected_changes.clone(),
                                unchanged: task_contract.unchanged,
                                validation: task_contract.validation,
                            };
                            contract.active_blockers = contract
                                .unresolved
                                .iter()
                                .chain(contract.regressions.iter())
                                .cloned()
                                .collect();
                            contract.resolved_blockers = contract.regression_constraints.clone();
                            if let Some(run) = summary.run.as_ref()
                                && let Some((_, evidence)) = db.load_worker_protocol(run.id)?
                            {
                                contract.current_persisted_execution_evidence = evidence;
                            }
                            contract.validation_failures = Vec::new();
                            if let Some(feedback) = &result.revision_feedback
                                && !feedback.trim().is_empty()
                            {
                                contract.reviewer_revision_feedback.push(feedback.clone());
                            }
                            Some(serde_json::to_string(&contract)?)
                        } else {
                            None
                        };
                        db.commit_task_review_result(
                            &summary.task.id,
                            run,
                            &result.blockers,
                            contract.as_deref(),
                            result.verdict.eq_ignore_ascii_case("pass"),
                            &persisted_output,
                            execution.token_usage,
                        )?;
                    } else {
                        db.update_agent_run_status_with_usage(
                            run,
                            "completed",
                            Some(&persisted_output),
                            execution.token_usage,
                        )?;
                    }
                    Ok((run, result))
                }
                Err(error) => {
                    db.update_agent_run_failure(
                        run,
                        Some(&execution.output),
                        &error.to_string(),
                        execution.token_usage,
                    )?;
                    Err(error)
                }
            }
        }
        Err(error) => {
            fail_run(db, run, &error, None)?;
            Err(error)
        }
    }
}

pub fn run_plan(
    db: &Database,
    request: &PlanningRequest,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
) -> Result<(i64, PlanResponse)> {
    request.validate()?;
    let (agent, resolved) = resolve_action_with_transport(
        db,
        AgentAction::Plan,
        overrides,
        backend.transport_eligibility(),
        backend.quota_refresher(),
    )?;
    let run = start_run(db, AgentAction::Plan, &resolved)?;
    let _run_finalizer = db.run_finalizer(run);
    let packet = planner_packet(db, request)?;
    let prompt = planner_prompt(&packet)?;
    let planner_backend = PlannerActionBackend::new(backend);
    let execution = invoke_action(db, run, &planner_backend, &agent, &resolved, &prompt);
    match execution {
        Ok(execution) => {
            let parsed =
                parse_structured::<PlanResponse>(&execution.output, "planner").and_then(|plan| {
                    plan.validate()?;
                    Ok(plan)
                });
            match parsed {
                Ok(plan) => {
                    db.update_agent_run_status_with_usage(
                        run,
                        "completed",
                        Some(&execution.output),
                        execution.token_usage,
                    )?;
                    Ok((run, plan))
                }
                Err(error) => {
                    db.update_agent_run_failure(
                        run,
                        Some(&execution.output),
                        &error.to_string(),
                        execution.token_usage,
                    )?;
                    Err(error)
                }
            }
        }
        Err(error) => {
            fail_run(db, run, &error, None)?;
            Err(error)
        }
    }
}

fn planner_prompt(packet: &serde_json::Value) -> Result<String> {
    crate::execution_packet::render_packet(
        &format!(
            "Produce a plan from this bounded authoritative Planner packet. Return only a PlanResponse JSON document and do not mutate project state. For every task, supply execution_hints.effort (low, medium, or high) only as non-authoritative metadata describing expected execution depth; Orc's shared economy resolver makes the final agent, model, effort, and tier decision. Do not infer or raise the hint mechanically from risk_factors. Give a concise effort_reason based on semantic complexity, coupling, and uncertainty rather than risk labels or description length. {} Declare risk factors accurately, make acceptance criteria, required tests, and validation precise enough to satisfy their deterministic safeguards, and decompose work when a task is too broad for reliable bounded execution.",
            planner_risk_guidance()
        ),
        packet,
    )
}

fn planner_risk_guidance() -> String {
    let mappings = crate::protocol::TaskRiskFactor::ALL
        .into_iter()
        .map(|risk| {
            format!(
                "{} -> {} ({})",
                risk.as_str(),
                risk.required_guard().as_str(),
                risk.required_guard().requirement()
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "Risk factors describe engineering risk; they never select or promote model tier or reasoning effort. Their deterministic guard mapping is: {mappings}. If no risk factor applies, do not invent one. Risk increases deterministic rigor and evidence, not model strength."
    )
}

struct LeadActionAdapter<'a> {
    db: &'a Database,
    run: i64,
    backend: &'a dyn ActionBackend,
    agent: &'a AgentDefinition,
    resolved: &'a ResolvedAction,
    usage: RefCell<Option<TokenUsage>>,
    output: RefCell<Option<String>>,
}

impl LeadBackend for LeadActionAdapter<'_> {
    fn invoke(&self, context: &LeadContext, message: &str) -> Result<LeadBackendResponse, String> {
        let prompt = crate::execution_packet::render_packet(
            "Act as Orc's project Lead using only the bounded authoritative Lead packet. Return only JSON matching {\"message\":string,\"proposals\":array,\"decision\":{\"kind\":\"DIRECT_TASKS\"|\"PLAN_REQUIRED\"|\"USER_DECISION_REQUIRED\"|\"APPROVE\"|\"REVISE_PLAN\",\"details\":object}}. Return exactly one decision. Proposals are human-gated and must not be applied.",
            &build_lead_packet(context, message),
        )
        .map_err(|error| error.to_string())?;
        let execution = invoke_action(
            self.db,
            self.run,
            self.backend,
            self.agent,
            self.resolved,
            &prompt,
        )
        .map_err(|error| error.to_string())?;
        self.usage.replace(execution.token_usage);
        self.output.replace(Some(execution.output.clone()));
        let response = crate::lead::parse_lead_transport_response(&execution.output)?;
        if response.decision.is_none() {
            return Err(
                "Lead provider response must contain exactly one structured decision".into(),
            );
        }
        Ok(response)
    }
}

pub fn run_lead(
    db: &Database,
    repo: &Path,
    message: &str,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
) -> Result<(i64, LeadResponse)> {
    let (agent, resolved) = resolve_action_with_transport(
        db,
        AgentAction::Lead,
        overrides,
        backend.transport_eligibility(),
        backend.quota_refresher(),
    )?;
    let run = start_run(db, AgentAction::Lead, &resolved)?;
    let _run_finalizer = db.run_finalizer(run);
    let adapter = LeadActionAdapter {
        db,
        run,
        backend,
        agent: &agent,
        resolved: &resolved,
        usage: RefCell::new(None),
        output: RefCell::new(None),
    };
    match LeadService::new(db, repo).invoke_with_run_id(message, &adapter, 50, Some(run)) {
        Ok(response) => {
            let output = adapter
                .output
                .borrow()
                .clone()
                .unwrap_or(serde_json::to_string(&response)?);
            db.update_agent_run_status_with_usage(
                run,
                "completed",
                Some(&output),
                *adapter.usage.borrow(),
            )?;
            Ok((run, response))
        }
        Err(error) => {
            db.update_agent_run_failure(
                run,
                adapter.output.borrow().as_deref(),
                &error.to_string(),
                *adapter.usage.borrow(),
            )?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lead::{LeadDecisionKind, LeadProposalKind};
    use crate::registry::{AUTOMATED, AVAILABLE};
    use crate::storage::db::LeadDecisionMetadata;
    use crate::task::TaskPriority;
    use std::collections::BTreeSet;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::{TempDir, tempdir};

    fn assert_codex_schema_compatible(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                assert!(!object.contains_key("oneOf"), "unsupported oneOf: {value}");
                assert!(
                    !object.contains_key("uniqueItems"),
                    "unsupported uniqueItems: {value}"
                );
                if let Some(properties) = object
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                {
                    assert_eq!(
                        object.get("additionalProperties"),
                        Some(&serde_json::Value::Bool(false))
                    );
                    let property_names = properties.keys().cloned().collect::<BTreeSet<_>>();
                    let required_names = object
                        .get("required")
                        .and_then(serde_json::Value::as_array)
                        .expect("Codex object schema must have required")
                        .iter()
                        .map(|value| value.as_str().unwrap().to_owned())
                        .collect::<BTreeSet<_>>();
                    assert_eq!(required_names, property_names);
                }
                for child in object.values() {
                    assert_codex_schema_compatible(child);
                }
            }
            serde_json::Value::Array(values) => {
                for child in values {
                    assert_codex_schema_compatible(child);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn codex_lead_schema_flattens_variants_and_requires_every_property() {
        let generic = schema(AgentAction::Lead);
        assert!(generic.contains("oneOf"));
        let adapted = crate::worker::codex_compatible_output_schema(&generic).unwrap();
        let value: serde_json::Value = serde_json::from_str(&adapted).unwrap();
        assert_codex_schema_compatible(&value);
        assert!(value.pointer("/properties/decision").is_some());
        assert!(
            value
                .pointer("/properties/proposals/items/properties/details/properties/task_id/type")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|types| types.iter().any(|value| value == "null"))
        );
    }

    #[test]
    fn codex_revision_schema_layers_claims_on_canonical_completion() {
        let adapted =
            crate::worker::codex_compatible_output_schema(&revision_handoff_schema()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&adapted).unwrap();
        assert_codex_schema_compatible(&value);
        assert!(
            value["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value == "completion")
        );
        assert!(
            value
                .pointer("/properties/completion/properties/step_results")
                .is_some()
        );
    }

    #[test]
    fn codex_planner_schema_omits_unique_items_but_keeps_array_bounds() {
        let value: serde_json::Value = serde_json::from_str(&schema(AgentAction::Plan)).unwrap();
        assert_codex_schema_compatible(&value);
        let task_properties = &value["properties"]["tasks"]["items"]["properties"];
        for field in [
            "expected_changes",
            "unchanged",
            "acceptance_criteria",
            "required_tests",
            "validation",
        ] {
            assert!(task_properties[field].get("uniqueItems").is_none());
        }
        assert_eq!(task_properties["expected_changes"]["minItems"], 1);
        assert_eq!(
            task_properties["expected_changes"]["maxItems"],
            crate::protocol::TaskProposal::MAX_EXPECTED_CHANGES
        );
        assert_eq!(task_properties["acceptance_criteria"]["minItems"], 1);
        assert_eq!(
            task_properties["acceptance_criteria"]["maxItems"],
            crate::protocol::TaskProposal::MAX_ACCEPTANCE_CRITERIA
        );
    }

    #[test]
    fn planner_prompt_separates_risk_guards_from_economy_selection() {
        let packet = serde_json::json!({"objective": "test planner guidance"});
        let prompt = planner_prompt(&packet).unwrap();

        assert!(prompt.contains(
            "Risk factors describe engineering risk; they never select or promote model tier or reasoning effort"
        ));
        assert!(prompt.contains("shared economy resolver makes the final agent, model, effort"));
        assert!(prompt.contains("Do not infer or raise the hint mechanically from risk_factors"));
        assert!(
            prompt.contains("Risk increases deterministic rigor and evidence, not model strength")
        );
    }

    #[test]
    fn planner_guidance_requires_guards_and_reliable_decomposition() {
        let guidance = planner_risk_guidance();

        for risk in crate::protocol::TaskRiskFactor::ALL {
            assert!(guidance.contains(risk.as_str()), "{}", risk.as_str());
            assert!(
                guidance.contains(risk.required_guard().as_str()),
                "{}",
                risk.as_str()
            );
            assert!(
                guidance.contains(risk.required_guard().requirement()),
                "{}",
                risk.as_str()
            );
        }
        let prompt = planner_prompt(&serde_json::json!({"objective": "bounded"})).unwrap();
        assert!(prompt.contains("acceptance criteria, required tests, and validation precise"));
        assert!(prompt.contains("decompose work when a task is too broad"));
    }

    #[test]
    fn every_codex_action_schema_uses_the_supported_object_subset() {
        for action in [
            AgentAction::Lead,
            AgentAction::Plan,
            AgentAction::Review,
            AgentAction::Code,
        ] {
            let adapted = crate::worker::codex_compatible_output_schema(&schema(action)).unwrap();
            let value: serde_json::Value = serde_json::from_str(&adapted).unwrap();
            assert_codex_schema_compatible(&value);
        }
        let adapted =
            crate::worker::codex_compatible_output_schema(&revision_handoff_schema()).unwrap();
        assert_codex_schema_compatible(&serde_json::from_str(&adapted).unwrap());
    }

    #[test]
    fn lead_proposal_variants_remain_semantically_strict_after_deserialization() {
        let proposal: LeadProposalKind = serde_json::from_value(serde_json::json!({
            "kind": "task",
            "details": {
                "local_id": "task-1", "title": "Incomplete", "objective": "Do work",
                "role": "developer", "priority": "normal", "depends_on": [],
                "capabilities": [], "scope_mode": null, "context_files": ["src/lib.rs"],
                "expected_changes": ["src/lib.rs"], "unchanged": [],
                "acceptance_criteria": [], "required_tests": ["cargo test"],
                "validation": ["cargo test"],
                "execution_hints": {"effort": "low", "effort_reason": "isolated"},
                "risk_factors": []
            }
        }))
        .unwrap();
        assert!(proposal.validate().is_err());

        let proposal: LeadProposalKind = serde_json::from_value(serde_json::json!({
            "kind": "revision", "details": {"task_id": "", "feedback": "fix it"}
        }))
        .unwrap();
        assert!(proposal.validate().is_err());
    }

    #[test]
    fn blocker_ids_are_stable_across_whitespace_and_case_rephrasing() {
        assert_eq!(
            blocker_id("Missing   validation evidence"),
            blocker_id("missing validation evidence")
        );
        assert!(blocker_id("missing validation evidence").starts_with("BLK-"));
    }

    fn handoff_contract() -> RevisionContract {
        RevisionContract {
            unresolved: vec![crate::storage::db::ReviewBlockerRecord {
                task_id: "T-1".into(),
                blocker_id: "BLK-1".into(),
                run_id: 1,
                requirement_ref: "R1".into(),
                evidence: "missing behavior".into(),
                severity: "high".into(),
                acceptance_condition: "behavior works".into(),
                status: "unresolved".into(),
                finding: "missing behavior".into(),
                first_seen: String::new(),
                last_seen: String::new(),
                blocker_key: "missing behavior".into(),
            }],
            regressions: Vec::new(),
            regression_constraints: Vec::new(),
            ..RevisionContract::default()
        }
    }

    #[test]
    fn addressed_handoff_rejects_vacuous_claim_without_changed_files() {
        let output = serde_json::json!({"claims": [{
            "blocker_id": "BLK-1", "status": "addressed",
            "implementation_summary": "fixed it", "changed_files": [],
            "unresolved_risk": null
        }]})
        .to_string();
        let error = validate_revision_handoff(&handoff_contract(), &output).unwrap_err();
        assert!(error.to_string().contains("changed files"));
    }

    #[test]
    fn handoff_rejects_placeholder_implementation_summary() {
        let output = serde_json::json!({"claims": [{
            "blocker_id": "BLK-1", "status": "addressed",
            "implementation_summary": "not tested", "changed_files": ["src/lib.rs"],
            "unresolved_risk": null
        }]})
        .to_string();
        let error = validate_revision_handoff(&handoff_contract(), &output).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing implementation evidence")
        );
    }

    fn claim_evidence_fixture(changed_path: &str) -> (crate::git::WorktreeChanges, String) {
        let changes = crate::git::WorktreeChanges {
            files: vec![crate::git::ChangedFile {
                status: "M".into(),
                path: changed_path.into(),
            }],
            stat: format!("{changed_path} | 1 +"),
            diff: format!(
                "diff --git a/{changed_path} b/{changed_path}\n--- a/{changed_path}\n+++ b/{changed_path}\n+fn covered() {{}}\n"
            ),
        };
        let handoff = serde_json::json!({"claims": [{
            "blocker_id": "BLK-1", "status": "addressed",
            "implementation_summary": "Added deterministic lifecycle coverage.",
            "changed_files": [changed_path],
            "unresolved_risk": null
        }]})
        .to_string();
        (changes, handoff)
    }

    // Revision handoff validation is structural; Orc runs deterministic
    // validation after the revision and before semantic Review. These tests
    // cover the handoff contract: claims must be structurally sound and tied
    // to real changed files, without provider-proven validation evidence.

    #[test]
    fn claim_without_current_change_evidence_is_rejected() {
        let (_changes, handoff) = claim_evidence_fixture("tests/lifecycle.rs");
        let error = validate_revision_handoff_with_evidence(&handoff_contract(), &handoff, None)
            .unwrap_err();
        assert!(error.to_string().contains("current change evidence"));
    }

    #[test]
    fn claimed_file_not_in_current_diff_is_rejected() {
        let (changes, handoff) = claim_evidence_fixture("tests/other.rs");
        let handoff = handoff.replace("tests/other.rs", "tests/lifecycle.rs");
        let error =
            validate_revision_handoff_with_evidence(&handoff_contract(), &handoff, Some(&changes))
                .unwrap_err();
        assert!(error.to_string().contains("file not changed"));
    }

    #[test]
    fn empty_implementation_summary_is_rejected() {
        let (changes, handoff) = claim_evidence_fixture("tests/lifecycle.rs");
        let handoff = handoff.replace(
            "\"implementation_summary\":\"Added deterministic lifecycle coverage.\"",
            "\"implementation_summary\":\"\"",
        );
        let error =
            validate_revision_handoff_with_evidence(&handoff_contract(), &handoff, Some(&changes))
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing implementation evidence")
        );
    }

    #[test]
    fn well_formed_claim_tied_to_real_changed_files_is_accepted() {
        let (changes, handoff) = claim_evidence_fixture("tests/lifecycle.rs");
        validate_revision_handoff_with_evidence(&handoff_contract(), &handoff, Some(&changes))
            .unwrap();
    }

    type Invocation = (AgentAction, Option<String>, Option<ReasoningEffort>);

    struct FakeBackend {
        calls: RefCell<Vec<Invocation>>,
        output: String,
    }

    impl ActionBackend for FakeBackend {
        fn invoke(
            &self,
            _agent: &AgentDefinition,
            action: AgentAction,
            _input: &str,
            model: Option<&str>,
            effort: Option<ReasoningEffort>,
        ) -> Result<ActionExecution> {
            self.calls
                .borrow_mut()
                .push((action, model.map(str::to_owned), effort));
            let output = match action {
                AgentAction::Plan => serde_json::json!({
                    "protocol_version": crate::protocol::PROTOCOL_VERSION,
                    "objective": "proposed",
                    "assumptions": [],
                    "risks": [],
                    "questions": [],
                    "tasks": []
                }),
                AgentAction::Lead => serde_json::json!({
                    "message": "proposal only",
                    "proposals": [{
                        "kind": "approval_request",
                        "details": {"reason": "decision", "details": "operator decides"}
                    }],
                    "decision": {"kind": "USER_DECISION_REQUIRED", "details": {}}
                }),
                AgentAction::Review => serde_json::json!({
                    "verdict": "revise",
                    "findings": ["missing coverage"],
                    "severity": "medium",
                    "revision_feedback": "add a test"
                }),
                AgentAction::Code => unreachable!(),
            };
            Ok(ActionExecution {
                output: if action == AgentAction::Review {
                    self.output.clone()
                } else {
                    output.to_string()
                },
                token_usage: Some(TokenUsage {
                    total_tokens: 30,
                    input_tokens: Some(20),
                    output_tokens: Some(10),
                    cached_input_tokens: None,
                }),
            })
        }
    }

    fn agent() -> AgentDefinition {
        AgentDefinition {
            id: "multi".into(),
            backend: "codex".into(),
            execution_mode: AUTOMATED.into(),
            display_name: "Multi".into(),
            enabled: true,
            priority: 10,
            capabilities: vec!["command_execution".into()],
            status: AVAILABLE.into(),
            unavailable_reason: None,
            profile_path: Some("/profile".into()),
            model: Some("default-model".into()),
            reasoning_effort: Some(ReasoningEffort::Low),
            config_metadata: None,
            quota_remaining_percent: None,
            quota_reset_at: None,
            quota_checked_at: None,
            quota_source: None,
            quota_limits: None,
            actions: vec![
                AgentAction::Code,
                AgentAction::Plan,
                AgentAction::Lead,
                AgentAction::Review,
            ],
        }
    }

    #[cfg(unix)]
    #[test]
    fn planner_production_boundary_is_read_only_and_returns_canonical_plan() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("orc.db");
        let repo = directory.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(repo.join("project.txt"), "unchanged").unwrap();
        let profile = directory.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let codex = bin.join("codex");
        let event = directory.path().join("event.jsonl");
        let proposal = serde_json::json!({"local_id":"inspect-repo","title":"Inspect repository","objective":"Inspect the persisted project","role":"developer","priority":"normal","depends_on":[],"capabilities":["inspection"],"scope_mode":"focused","context_files":["project.txt"],"expected_changes":["project.txt"],"unchanged":["task state"],"acceptance_criteria":["context is inspected"],"required_tests":["cargo test"],"validation":["cargo test"],"execution_hints":{"effort":"low","effort_reason":"isolated inspection"},"risk_factors":[]});
        let response = serde_json::json!({"protocol_version":1,"objective":"inspect","assumptions":["lead-approved"],"risks":[],"questions":[],"tasks":[proposal]});
        std::fs::write(&event, serde_json::json!({"type":"item.completed", "item":{"type":"agent_message", "text": response.to_string()}}).to_string() + "\n").unwrap();
        let args_file = directory.path().join("args");
        std::fs::write(&codex, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\ninput=$(cat)\necho \"$input\" | grep -q persisted-context || exit 9\necho \"$input\" | grep -q lead-decision || exit 10\ncat {}\n", args_file.display(), event.display())).unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("project").unwrap();
        db.record_lead_turn(
            project,
            crate::lead::LeadRole::Assistant,
            "persisted Lead context",
        )
        .unwrap();
        db.record_lead_decision(
            project,
            &LeadDecisionKind::PlanRequired,
            &serde_json::json!({"message":"lead-decision"}),
            LeadDecisionMetadata {
                snapshot: "persisted snapshot",
                run_id: None,
                source_request: "request",
                summary: "lead-decision",
            },
        )
        .unwrap();
        db.insert_agent(&AgentDefinition {
            profile_path: Some(profile.display().to_string()),
            ..agent()
        })
        .unwrap();
        let request: PlanningRequest = serde_json::from_value(serde_json::json!({
            "protocol_version": crate::protocol::PROTOCOL_VERSION, "kind":"project_plan", "project":{"name":"persisted-project","repository":"repo","branch":null,"commit":null},
            "engineering_contract":"persisted-context", "objective":"inspect", "constraints":[], "target_platforms":[], "stack":[], "non_goals":[], "deliverables":[], "definition_of_done":[],
            "response_schema": crate::protocol::PlanResponseSchema::v1(), "role_boundaries":["Planner cannot invoke Lead, dispatch, review, revise, accept, create, or apply"], "planning_constraints":[], "approval_requirements":[], "current_state":db.planning_project_state().unwrap(), "full_report":null
        })).unwrap();
        let before = std::fs::read(repo.join("project.txt")).unwrap();
        let backend = WorkerActionBackend::new(&repo).with_planner_executable(&codex);
        let (_, plan) = run_plan(&db, &request, &ActionOverrides::default(), &backend).unwrap();
        assert_eq!(plan.objective, "inspect");
        assert_eq!(plan.tasks[0].local_id, "inspect-repo");
        assert_eq!(plan.tasks[0].expected_changes, vec!["project.txt"]);
        let args = std::fs::read_to_string(args_file).unwrap();
        assert!(args.lines().any(|arg| arg == "--sandbox"));
        assert!(args.lines().any(|arg| arg == "read-only"));
        assert_eq!(std::fs::read(repo.join("project.txt")).unwrap(), before);
        assert!(db.list_tasks().unwrap().is_empty());
        assert_eq!(
            db.list_agent_runs(project, 10).unwrap()[0].status,
            "completed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn planner_production_boundary_rejects_malformed_proposal_without_mutation() {
        let directory = tempdir().unwrap();
        let profile = directory.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let bin = directory.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let codex = bin.join("codex");
        std::fs::write(&codex, "#!/bin/sh\ncat >/dev/null\necho '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"{\\\"protocol_version\\\":1,\\\"objective\\\":\\\"bad\\\",\\\"assumptions\\\":[],\\\"risks\\\":[],\\\"questions\\\":[],\\\"tasks\":[{\\\"local_id\\\":\\\"t1\\\",\\\"title\\\":\\\"\\\",\\\"objective\\\":\\\"x\\\",\\\"role\\\":\\\"coder\\\",\\\"priority\\\":\\\"normal\\\",\\\"depends_on\\\":[],\\\"capabilities\\\":[],\\\"scope_mode\\\":null,\\\"context_files\\\":[],\\\"expected_changes\\\":[],\\\"unchanged\\\":[],\\\"acceptance_criteria\\\":[],\\\"required_tests\\\":[],\\\"validation\\\":[],\\\"execution_hints\\\":{}}]}\"}}}'\n").unwrap();
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let project = db.create_project("project").unwrap();
        db.insert_agent(&AgentDefinition {
            profile_path: Some(profile.display().to_string()),
            ..agent()
        })
        .unwrap();
        let request: PlanningRequest = serde_json::from_value(serde_json::json!({"protocol_version":1,"kind":"project_plan","project":null,"engineering_contract":"","objective":"plan","constraints":[],"target_platforms":[],"stack":[],"non_goals":[],"deliverables":[],"definition_of_done":[],"response_schema":crate::protocol::PlanResponseSchema::v1(),"role_boundaries":[],"planning_constraints":[],"approval_requirements":[],"current_state":null,"full_report":null})).unwrap();
        assert!(
            run_plan(
                &db,
                &request,
                &ActionOverrides::default(),
                &WorkerActionBackend::new(directory.path()).with_planner_executable(&codex)
            )
            .is_err()
        );
        assert!(db.list_tasks().unwrap().is_empty());
        assert_eq!(db.list_agent_runs(project, 10).unwrap()[0].status, "failed");
    }

    #[test]
    fn planner_boundary_rejects_every_orchestration_action_at_the_backend_seam() {
        struct RecordingBackend {
            attempted: RefCell<Vec<AgentAction>>,
        }

        impl ActionBackend for RecordingBackend {
            fn invoke(
                &self,
                _agent: &AgentDefinition,
                action: AgentAction,
                _input: &str,
                _model: Option<&str>,
                _effort: Option<ReasoningEffort>,
            ) -> Result<ActionExecution> {
                self.attempted.borrow_mut().push(action);
                Ok(ActionExecution {
                    output: String::new(),
                    token_usage: None,
                })
            }
        }

        let inner = RecordingBackend {
            attempted: RefCell::new(Vec::new()),
        };
        let boundary = PlannerActionBackend::new(&inner);
        let planner = agent();
        for action in [AgentAction::Lead, AgentAction::Code, AgentAction::Review] {
            let error = boundary
                .invoke(
                    &planner,
                    action,
                    "provider attempted orchestration",
                    None,
                    None,
                )
                .unwrap_err();
            assert!(error.to_string().contains("rejects orchestration action"));
        }
        assert!(inner.attempted.borrow().is_empty());
        boundary
            .invoke(&planner, AgentAction::Plan, "plan only", None, None)
            .unwrap();
        assert_eq!(inner.attempted.borrow().as_slice(), &[AgentAction::Plan]);
    }

    #[test]
    fn one_agent_runs_all_actions_with_profiles_overrides_gates_and_usage() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("orc.db");
        std::fs::create_dir(directory.path().join(".orc")).unwrap();
        std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("project").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        db.update_task_status(&task, crate::task::TaskStatus::Review)
            .unwrap();
        db.insert_agent(&agent()).unwrap();
        db.set_agent_action_profile(
            "multi",
            AgentAction::Plan,
            Some("plan-model"),
            Some(ReasoningEffort::High),
        )
        .unwrap();
        drop(db);

        let app = crate::app::OrcApp::open(&db_path, directory.path()).unwrap();
        let backend = FakeBackend {
            calls: RefCell::new(Vec::new()),
            output: serde_json::json!({
                "verdict": "revise",
                "findings": ["missing coverage"],
                "blocking_findings": [],
                "non_blocking_findings": [],
                "severity": "medium",
                "revision_feedback": "add a test"
            })
            .to_string(),
        };
        let request = app.planning_request().unwrap();
        app.automated_plan_with_backend(&request, &ActionOverrides::default(), &backend)
            .unwrap();
        let lead = app
            .automated_lead_with_backend(
                "advise",
                &ActionOverrides {
                    agent_id: Some("multi".into()),
                    model: Some("lead-override".into()),
                    reasoning_effort: Some(ReasoningEffort::Medium),
                },
                &backend,
            )
            .unwrap()
            .1;
        assert_eq!(lead.proposals.len(), 1);
        assert_eq!(app.approvals().unwrap().len(), 0);
        let review = app
            .automated_review_with_backend(
                &task,
                &ActionOverrides::default(),
                &backend,
                &RecordingValidationRunner::new(&[]),
            )
            .unwrap()
            .1;
        assert_eq!(review.verdict, "revise");
        assert_eq!(
            app.task(&task).unwrap().unwrap().status.to_string(),
            "revision_required"
        );
        let calls = backend.calls.borrow();
        assert_eq!(calls[0].1.as_deref(), Some("plan-model"));
        assert_eq!(calls[0].2, Some(ReasoningEffort::High));
        assert_eq!(calls[1].1.as_deref(), Some("lead-override"));
        assert_eq!(calls[1].2, Some(ReasoningEffort::Medium));
        drop(calls);
        drop(app);

        let reopened = Database::open(&db_path).unwrap();
        assert!(
            reopened
                .actionable_revision_contract(&task)
                .unwrap()
                .is_some()
        );
        let runs = reopened.list_agent_runs(project, 10).unwrap();
        assert_eq!(runs.len(), 3);
        assert!(runs.iter().all(|run| run.status == "completed"));
        assert!(
            runs.iter()
                .all(|run| reopened.resolution_records(run.id).unwrap().len() == 1)
        );
        assert!(runs.iter().all(|run| {
            let record = reopened.resolution_records(run.id).unwrap().remove(0);
            record.selected_agent == "multi"
                && record.selected_model.as_deref() == run.resolved_model.as_deref()
                && record.effort == run.resolved_reasoning_effort
        }));
        assert!(runs.iter().all(|run| {
            reopened
                .get_worker_result(run.id)
                .unwrap()
                .is_some_and(|result| result.total_tokens == Some(30))
        }));
    }

    fn review_fixture(output: serde_json::Value) -> (Database, ReviewSummary, FakeBackend) {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("orc.db");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("project").unwrap();
        let task = db
            .insert_task(
                project,
                "task",
                "objective",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        db.insert_agent(&agent()).unwrap();
        db.update_task_status(&task, crate::task::TaskStatus::Review)
            .unwrap();
        let task = db.get_task(&task).unwrap().unwrap();
        let summary = ReviewSummary {
            task,
            run: None,
            result: None,
            worktree_path: None,
            changes: crate::git::WorktreeChanges::default(),
            change_evidence: None,
            validation_evidence: None,
            prior_reviews: Vec::new(),
            automated_reviews: Vec::new(),
        };
        (
            db,
            summary,
            FakeBackend {
                calls: RefCell::new(Vec::new()),
                output: output.to_string(),
            },
        )
    }

    #[test]
    fn task_review_accepts_every_open_status_and_rejects_terminal_statuses() {
        let output = serde_json::json!({
            "verdict": "PASS", "findings": [], "blocking_findings": [],
            "non_blocking_findings": [], "severity": null, "revision_feedback": null,
            "blockers": []
        });
        for status in [
            crate::task::TaskStatus::Ready,
            crate::task::TaskStatus::Active,
            crate::task::TaskStatus::Review,
            crate::task::TaskStatus::Blocked,
            crate::task::TaskStatus::RevisionRequired,
            crate::task::TaskStatus::AcceptanceReady,
        ] {
            let (db, mut summary, backend) = review_fixture(output.clone());
            db.update_task_status(&summary.task.id, status).unwrap();
            summary.task.status = status;
            run_review(
                &db,
                &summary,
                &ActionOverrides::default(),
                &backend,
                Path::new("."),
                &RecordingValidationRunner::new(&[]),
            )
            .unwrap();
            assert_eq!(
                db.get_task(&summary.task.id).unwrap().unwrap().status,
                crate::task::TaskStatus::AcceptanceReady,
                "{status} should be directly reviewable"
            );
        }

        for status in [
            crate::task::TaskStatus::Done,
            crate::task::TaskStatus::Cancelled,
        ] {
            let (db, mut summary, backend) = review_fixture(output.clone());
            db.update_task_status(&summary.task.id, status).unwrap();
            summary.task.status = status;
            let error = run_review(
                &db,
                &summary,
                &ActionOverrides::default(),
                &backend,
                Path::new("."),
                &RecordingValidationRunner::new(&[]),
            )
            .unwrap_err();
            assert!(error.to_string().contains("terminal status"));
            assert!(
                db.list_agent_runs(db.get_project_id().unwrap().unwrap(), 10)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn manually_repaired_revision_required_task_can_be_reviewed_without_revise() {
        let (db, mut summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null,
                "blockers": []
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        db.update_task_status(&summary.task.id, crate::task::TaskStatus::RevisionRequired)
            .unwrap();
        summary.task.status = crate::task::TaskStatus::RevisionRequired;
        let runner = RecordingValidationRunner::new(&[]);

        run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();

        assert!(runner.executed().is_empty());
        assert_eq!(backend.calls.borrow().len(), 1);
        assert_eq!(
            db.get_task(&summary.task.id).unwrap().unwrap().status,
            crate::task::TaskStatus::AcceptanceReady
        );
    }

    /// Records every command it was asked to run, exactly once each call, so
    /// tests can assert both which commands were selected and that none ran
    /// more than once per review.
    struct RecordingValidationRunner {
        fail_on: Vec<String>,
        executed: std::sync::Mutex<Vec<String>>,
        failure_output: Option<String>,
    }

    impl RecordingValidationRunner {
        fn new(fail_on: &[&str]) -> Self {
            Self {
                fail_on: fail_on.iter().map(|value| (*value).to_owned()).collect(),
                executed: std::sync::Mutex::new(Vec::new()),
                failure_output: None,
            }
        }

        fn with_failure_output(fail_on: &[&str], output: String) -> Self {
            Self {
                fail_on: fail_on.iter().map(|value| (*value).to_owned()).collect(),
                executed: std::sync::Mutex::new(Vec::new()),
                failure_output: Some(output),
            }
        }

        fn executed(&self) -> Vec<String> {
            self.executed.lock().unwrap().clone()
        }
    }

    impl ValidationRunner for RecordingValidationRunner {
        fn run(
            &self,
            command: &str,
            _working_dir: &Path,
        ) -> Result<crate::validation::ValidationStepResult> {
            self.executed.lock().unwrap().push(command.to_owned());
            let passed = !self.fail_on.iter().any(|value| value == command);
            Ok(crate::validation::ValidationStepResult {
                command: command.to_owned(),
                category: if passed {
                    crate::validation::ValidationCategory::Success
                } else {
                    crate::validation::ValidationCategory::Test
                },
                passed,
                stdout: String::new(),
                stderr: if passed {
                    String::new()
                } else {
                    self.failure_output
                        .clone()
                        .unwrap_or_else(|| format!("{command} failed"))
                },
                exit_status: Some(if passed { 0 } else { 1 }),
                diagnostics: None,
                failure_classification: (!passed)
                    .then_some(crate::validation::ValidationFailureClassification::Implementation),
                fallback_command: None,
            })
        }
    }

    /// A review fixture with a real worktree directory configured with the
    /// given `.orc/validation.toml` and changed files, for exercising
    /// [`run_review`].
    fn validation_review_fixture(
        output: serde_json::Value,
        validation_toml: &str,
        changed_files: &[&str],
    ) -> (Database, ReviewSummary, FakeBackend, TempDir) {
        let (db, mut summary, backend) = review_fixture(output);
        let directory = tempdir().unwrap();
        let worktree_rel = "worktree";
        let worktree_dir = directory.path().join(worktree_rel);
        std::fs::create_dir_all(worktree_dir.join(".orc")).unwrap();
        std::fs::write(worktree_dir.join(".orc/validation.toml"), validation_toml).unwrap();
        summary.worktree_path = Some(worktree_rel.to_owned());
        summary.changes = crate::git::WorktreeChanges {
            files: changed_files
                .iter()
                .map(|path| crate::git::ChangedFile {
                    status: "M".into(),
                    path: (*path).into(),
                })
                .collect(),
            stat: String::new(),
            diff: String::new(),
        };
        (db, summary, backend, directory)
    }

    const GROUPED_VALIDATION_TOML: &str = r#"
commands = ["cargo test"]

[[groups]]
name = "rust-core"
commands = ["cargo fmt --check", "cargo test"]

[[groups]]
name = "frontend"
commands = ["npm run typecheck", "npm run build"]
"#;

    #[test]
    fn review_consumes_validation_evidence_without_executing_commands() {
        let (db, summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let runner = RecordingValidationRunner::new(&[]);
        let (run_id, result) = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();
        assert_eq!(result.verdict, "PASS");
        assert!(runner.executed().is_empty());
        assert!(
            db.latest_validation_result_for_run(run_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn review_rejects_stale_manual_validation_before_provider_invocation() {
        let (db, mut summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [],
                "non_blocking_findings": [], "severity": null,
                "revision_feedback": null, "blockers": []
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let project = db.get_project_id().unwrap().unwrap();
        let run = db
            .create_agent_run_with_mode(project, &summary.task.id, "multi", crate::registry::MANUAL)
            .unwrap();
        db.store_worktree_metadata(run, &summary.task.id, "branch", "worktree")
            .unwrap();
        let report = crate::validation::ValidationReport {
            steps: vec![crate::validation::ValidationStepResult {
                command: "cargo test".into(),
                category: crate::validation::ValidationCategory::Success,
                passed: true,
                stdout: String::new(),
                stderr: String::new(),
                exit_status: Some(0),
                diagnostics: None,
                failure_classification: None,
                fallback_command: None,
            }],
        };
        let report_json = serde_json::to_string(&report).unwrap();
        db.record_lifecycle_event(
            "validation_result",
            Some(&summary.task.id),
            Some(run),
            Some("multi"),
            Some(&report_json),
        )
        .unwrap();
        let fingerprint = revision_worktree_fingerprint(&summary.changes);
        db.record_lifecycle_event(
            "validation_selection",
            Some(&summary.task.id),
            Some(run),
            Some("multi"),
            Some(&serde_json::json!({"worktree_fingerprint": fingerprint}).to_string()),
        )
        .unwrap();
        summary.run = db.get_agent_run(run).unwrap();
        summary.validation_evidence = Some(report_json);
        summary.changes.diff = "new current worktree state".into();

        let error = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &RecordingValidationRunner::new(&[]),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stale"));
        assert!(backend.calls.borrow().is_empty());
    }

    #[test]
    fn review_rejects_missing_manual_validation_before_provider_invocation() {
        let (db, mut summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [],
                "non_blocking_findings": [], "severity": null,
                "revision_feedback": null, "blockers": []
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let project = db.get_project_id().unwrap().unwrap();
        let run = db
            .create_agent_run_with_mode(
                project,
                &summary.task.id,
                "manual-coder",
                crate::registry::MANUAL,
            )
            .unwrap();
        db.store_worktree_metadata(run, &summary.task.id, "branch", "worktree")
            .unwrap();
        summary.run = db.get_agent_run(run).unwrap();
        summary.validation_evidence = None;

        let error = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &RecordingValidationRunner::new(&[]),
        )
        .unwrap_err();

        assert!(error.to_string().contains("requires current passing"));
        assert!(backend.calls.borrow().is_empty());
    }

    #[test]
    fn rust_core_task_review_does_not_run_frontend_validation() {
        let (db, summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null
            }),
            GROUPED_VALIDATION_TOML,
            &["src/storage/db.rs"],
        );
        let runner = RecordingValidationRunner::new(&[]);
        run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();
        assert!(
            !runner
                .executed()
                .iter()
                .any(|command| command.starts_with("npm"))
        );
    }

    #[test]
    fn frontend_task_review_does_not_run_rust_validation() {
        let (db, summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null
            }),
            GROUPED_VALIDATION_TOML,
            &["src/App.vue", "package.json"],
        );
        let runner = RecordingValidationRunner::new(&[]);
        run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();
        assert!(runner.executed().is_empty());
    }

    #[test]
    fn failed_task_specific_validation_becomes_a_focused_blocker_even_when_reviewer_says_pass() {
        let (db, summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [], "blockers": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let runner = RecordingValidationRunner::new(&["cargo fmt --check"]);
        let (run_id, result) = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();
        assert_eq!(result.verdict, "PASS");
        assert!(result.blockers.is_empty());
        assert!(runner.executed().is_empty());
        assert!(
            db.latest_validation_result_for_run(run_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn every_failed_review_validation_command_becomes_a_blocker() {
        let (db, summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [], "blockers": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let runner = RecordingValidationRunner::new(&["cargo fmt --check", "cargo test"]);
        let (run_id, result) = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();

        assert_eq!(result.verdict, "PASS");
        assert!(result.blockers.is_empty());
        assert!(runner.executed().is_empty());
        assert!(
            db.latest_validation_result_for_run(run_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn reviewer_validation_summary_is_bounded_after_collecting_all_results() {
        struct CapturingBackend(RefCell<String>);

        impl ActionBackend for CapturingBackend {
            fn invoke(
                &self,
                _agent: &AgentDefinition,
                action: AgentAction,
                input: &str,
                _model: Option<&str>,
                _effort: Option<ReasoningEffort>,
            ) -> Result<ActionExecution> {
                assert_eq!(action, AgentAction::Review);
                *self.0.borrow_mut() = input.to_owned();
                Ok(ActionExecution {
                    output: serde_json::json!({
                        "verdict": "PASS", "findings": [], "blocking_findings": [], "blockers": [],
                        "non_blocking_findings": [], "severity": null, "revision_feedback": null
                    })
                    .to_string(),
                    token_usage: None,
                })
            }
        }

        let (db, summary, _backend, directory) = validation_review_fixture(
            serde_json::json!({}),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let backend = CapturingBackend(RefCell::new(String::new()));
        let runner = RecordingValidationRunner::with_failure_output(
            &["cargo fmt --check", "cargo test"],
            "diagnostic ".repeat(1_000),
        );
        run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();

        let prompt = backend.0.borrow();
        assert!(!prompt.contains("diagnostic diagnostic"));
        assert!(
            prompt.len() < 10_000,
            "review prompt was not bounded: {}",
            prompt.len()
        );
    }

    #[test]
    fn recurring_validation_failure_reopens_a_previously_resolved_blocker_as_a_regression() {
        let (db, summary, backend, directory) = validation_review_fixture(
            serde_json::json!({
                "verdict": "PASS", "findings": [], "blocking_findings": [], "blockers": [],
                "non_blocking_findings": [], "severity": null, "revision_feedback": null
            }),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        let project_id = db.get_project_id().unwrap().unwrap();
        let seed_run = db
            .create_project_action_run(
                project_id,
                Some(summary.task.id.as_str()),
                "review",
                &agent().id,
                AgentRunExecution {
                    class: "review",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        db.commit_task_review_result(
            &summary.task.id,
            seed_run,
            &[ReviewBlocker {
                id: blocker_id("task-validation:cargo fmt --check"),
                prior_blocker_id: None,
                blocker_key: "task-validation:cargo fmt --check".into(),
                requirement_ref: "task-specific validation (rust-core)".into(),
                evidence: "previously failed".into(),
                severity: "high".into(),
                acceptance_condition: "`cargo fmt --check` must pass".into(),
                status: "resolved".into(),
                finding: "Task-specific validation command `cargo fmt --check` failed.".into(),
            }],
            None,
            true,
            "{}",
            None,
        )
        .unwrap();
        // The seeded PASS models an earlier review. A subsequent validation
        // recurrence belongs to a new, explicitly prepared review cycle.
        db.update_task_status(&summary.task.id, crate::task::TaskStatus::Review)
            .unwrap();

        let runner = RecordingValidationRunner::new(&["cargo fmt --check"]);
        let (_, result) = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();
        assert!(result.blockers.is_empty());
    }

    #[test]
    fn review_without_a_worktree_skips_validation() {
        let (db, summary, backend) = review_fixture(serde_json::json!({
            "verdict": "PASS", "findings": [], "blocking_findings": [],
            "non_blocking_findings": [], "severity": null, "revision_feedback": null
        }));
        let runner = RecordingValidationRunner::new(&[]);
        let directory = tempdir().unwrap();
        let (run_id, result) = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();
        assert_eq!(result.verdict, "PASS");
        assert!(runner.executed().is_empty());
        assert!(
            db.latest_validation_result_for_run(run_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn review_packet_size_is_independent_of_duplicated_read_model_history() {
        let (db, mut summary, _) = review_fixture(serde_json::json!({}));
        let baseline = serde_json::to_string(&review_packet(&db, &summary).unwrap()).unwrap();
        let historical = crate::review::PriorReview {
            run_id: 99,
            agent: "historical-reviewer".into(),
            status: "completed".into(),
            started_at: "then".into(),
            finished_at: Some("later".into()),
            model: Some("historical-model".into()),
            reasoning_effort: Some(ReasoningEffort::High),
            verdict: "REVISE".into(),
            severity: Some("high".into()),
            findings: vec!["duplicated historical context ".repeat(2_000)],
            blocking_findings: vec!["duplicated historical blocker ".repeat(2_000)],
            non_blocking_findings: Vec::new(),
            revision_feedback: Some("old feedback ".repeat(2_000)),
            validation_evidence: Some("old validation ".repeat(2_000)),
            blockers: Vec::new(),
        };
        summary.prior_reviews = vec![historical.clone(); 20];
        summary.automated_reviews = vec![historical; 20];
        let packet = review_packet(&db, &summary).unwrap();
        let expanded = serde_json::to_string(&packet).unwrap();
        assert_eq!(expanded.len(), baseline.len());
        assert!(packet.get("prior_reviews").is_none());
        assert!(packet.get("automated_reviews").is_none());
        assert!(packet.get("prior_blockers").is_some());
        assert!(packet.get("worktree_path").is_none());
    }

    #[test]
    fn review_invocation_has_no_command_execution_and_receives_orc_validation() {
        struct ReviewBoundaryBackend {
            capabilities: RefCell<Vec<String>>,
            prompt: RefCell<String>,
            observed: RefCell<Vec<String>>,
        }

        impl ActionBackend for ReviewBoundaryBackend {
            fn invoke(
                &self,
                agent: &AgentDefinition,
                action: AgentAction,
                input: &str,
                _model: Option<&str>,
                _effort: Option<ReasoningEffort>,
            ) -> Result<ActionExecution> {
                assert_eq!(action, AgentAction::Review);
                *self.capabilities.borrow_mut() = agent.capabilities.clone();
                *self.prompt.borrow_mut() = input.to_owned();
                Ok(ActionExecution {
                    output: serde_json::json!({
                        "verdict": "PASS", "findings": [], "blocking_findings": [],
                        "non_blocking_findings": [], "severity": null,
                        "revision_feedback": null, "blockers": []
                    })
                    .to_string(),
                    token_usage: None,
                })
            }

            fn observe(&self, message: &str) {
                self.observed.borrow_mut().push(message.to_owned());
            }
        }

        let (db, mut summary, _backend, directory) = validation_review_fixture(
            serde_json::json!({}),
            GROUPED_VALIDATION_TOML,
            &["src/agent.rs"],
        );
        db.update_task_status(&summary.task.id, crate::task::TaskStatus::Review)
            .unwrap();
        summary.task.status = crate::task::TaskStatus::Review;
        let backend = ReviewBoundaryBackend {
            capabilities: RefCell::new(Vec::new()),
            prompt: RefCell::new(String::new()),
            observed: RefCell::new(Vec::new()),
        };
        let runner = RecordingValidationRunner::new(&[]);
        run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            directory.path(),
            &runner,
        )
        .unwrap();

        assert!(runner.executed().is_empty());
        assert!(
            !backend
                .capabilities
                .borrow()
                .iter()
                .any(|capability| capability == "command_execution")
        );
        let prompt = backend.prompt.borrow();
        assert!(!prompt.contains("Selected groups: rust-core"));
        assert!(!prompt.contains("cargo fmt --check"));
        assert!(prompt.contains("Do not execute shell commands"));
        let observed = backend.observed.borrow();
        assert!(
            observed
                .iter()
                .any(|message| message == "Preparing review packet       OK")
        );
        assert!(
            observed
                .iter()
                .any(|message| message.starts_with("Starting reviewer"))
        );
        assert!(
            observed
                .iter()
                .any(|message| message == "Reviewing implementation      ...")
        );
        assert!(
            observed
                .iter()
                .any(|message| message == "Reviewer finished             PASS")
        );
    }

    #[test]
    fn normal_review_progress_filters_raw_provider_protocol_events() {
        assert!(normal_provider_progress("provider item.started: command_execution").is_none());
        assert!(normal_provider_progress("provider item.completed: command_execution").is_none());
        assert!(normal_provider_progress("provider turn.started").is_none());
        assert_eq!(
            normal_provider_progress("Reviewing implementation      ..."),
            Some("Reviewing implementation      ...")
        );
        assert_eq!(
            normal_provider_progress("cargo test                     PASS"),
            Some("cargo test                     PASS")
        );
    }

    #[test]
    fn later_pass_resolves_earlier_blocker_in_resolution_ledger() {
        let reviews = vec![
            crate::review::PriorReview {
                run_id: 1,
                agent: "reviewer".into(),
                status: "completed".into(),
                started_at: "".into(),
                finished_at: None,
                model: None,
                reasoning_effort: None,
                severity: None,
                findings: vec![],
                validation_evidence: None,
                blockers: Vec::new(),
                verdict: "REVISE".into(),
                blocking_findings: vec!["validation is incomplete".into()],
                non_blocking_findings: Vec::new(),
                revision_feedback: Some("complete validation".into()),
            },
            crate::review::PriorReview {
                run_id: 2,
                agent: "reviewer".into(),
                status: "completed".into(),
                started_at: "".into(),
                finished_at: None,
                model: None,
                reasoning_effort: None,
                severity: None,
                findings: vec![],
                validation_evidence: None,
                blockers: Vec::new(),
                verdict: "PASS".into(),
                blocking_findings: Vec::new(),
                non_blocking_findings: Vec::new(),
                revision_feedback: None,
            },
        ];

        let ledger = review_resolution_ledger(&reviews);

        assert!(ledger.contains("RESOLVED prior blockers"));
        assert!(ledger.contains("validation is incomplete"));
        assert!(!ledger.contains("UNRESOLVED prior blockers"));
    }

    #[test]
    fn resolution_prompt_keeps_equivalent_blockers_resolved_unless_evidence_shows_regression() {
        assert!(TASK_REVIEW_INSTRUCTIONS.contains(
            "equivalent or reworded findings refer to the same concern and remain resolved"
        ));
        assert!(TASK_REVIEW_INSTRUCTIONS
            .contains("Reopen a resolved concern only when supplied current evidence demonstrates a genuine regression"));
    }

    #[test]
    fn task_review_passes_with_non_blocking_findings() {
        let (db, summary, backend) = review_fixture(serde_json::json!({
            "verdict": "REVISE",
            "findings": ["unrelated project defect"],
            "blocking_findings": [],
            "non_blocking_findings": ["unrelated project defect"],
            "severity": "low",
            "revision_feedback": null
        }));
        let result = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            Path::new("."),
            &RecordingValidationRunner::new(&[]),
        )
        .unwrap()
        .1;
        assert_eq!(result.verdict, "PASS");
        assert_eq!(result.non_blocking_findings.len(), 1);
    }

    #[test]
    fn task_review_keeps_in_scope_and_regression_findings_blocking() {
        for finding in ["missing expected change", "regression introduced by task"] {
            let (db, summary, backend) = review_fixture(serde_json::json!({
                "verdict": "REVISE",
                "findings": [finding],
                "blocking_findings": [finding],
                "non_blocking_findings": [],
                "severity": "high",
                "revision_feedback": finding
            }));
            let result = run_review(
                &db,
                &summary,
                &ActionOverrides::default(),
                &backend,
                Path::new("."),
                &RecordingValidationRunner::new(&[]),
            )
            .unwrap()
            .1;
            assert_eq!(result.verdict, "REVISE");
            assert_eq!(result.blocking_findings, vec![finding]);
        }
    }

    #[test]
    fn project_review_can_report_broader_findings() {
        let (db, summary, backend) = review_fixture(serde_json::json!({
            "verdict": "REVISE",
            "findings": ["architectural debt"],
            "blocking_findings": [],
            "non_blocking_findings": ["architectural debt"],
            "severity": "low",
            "revision_feedback": null
        }));
        let result = run_project_review(&db, &summary, &ActionOverrides::default(), &backend)
            .unwrap()
            .1;
        assert_eq!(result.verdict, "REVISE");
        assert_eq!(result.non_blocking_findings, vec!["architectural debt"]);
    }

    #[test]
    fn task_review_run_is_associated_with_task() {
        let (db, summary, backend) = review_fixture(serde_json::json!({
            "verdict": "PASS",
            "findings": [],
            "blocking_findings": [],
            "non_blocking_findings": [],
            "severity": null,
            "revision_feedback": null
        }));
        let (run_id, _) = run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            Path::new("."),
            &RecordingValidationRunner::new(&[]),
        )
        .unwrap();
        let run = db.get_agent_run(run_id).unwrap().unwrap();
        assert_eq!(run.task_id.as_deref(), Some(summary.task.id.as_str()));
        assert_eq!(run.execution_class, "review");
    }

    #[test]
    fn review_context_includes_prior_reviews_without_selecting_them_as_implementation() {
        let (db, summary, backend) = review_fixture(serde_json::json!({
            "verdict": "REVISE",
            "findings": ["required behavior is missing"],
            "blocking_findings": ["required behavior is missing"],
            "non_blocking_findings": ["optional cleanup"],
            "severity": "high",
            "revision_feedback": "implement the required behavior"
        }));
        let project_id = db.get_project_id().unwrap().unwrap();
        let implementation_run = db
            .create_agent_run_with_execution(
                project_id,
                &summary.task.id,
                "multi",
                "automated",
                AgentRunExecution {
                    class: "code",
                    model: None,
                    effort: None,
                    source: "test",
                },
            )
            .unwrap();
        db.update_agent_run_status(implementation_run, "completed", Some("implemented"))
            .unwrap();
        run_review(
            &db,
            &summary,
            &ActionOverrides::default(),
            &backend,
            Path::new("."),
            &RecordingValidationRunner::new(&[]),
        )
        .unwrap();

        let next = crate::review::build_review(&db, &summary.task.id, Path::new(".")).unwrap();
        assert_eq!(next.run.unwrap().id, implementation_run);
        assert_eq!(next.prior_reviews.len(), 1);
        assert_eq!(
            next.prior_reviews[0].blocking_findings,
            vec!["required behavior is missing"]
        );
        assert_eq!(
            next.prior_reviews[0].revision_feedback.as_deref(),
            Some("implement the required behavior")
        );
    }

    #[test]
    fn malformed_output_fails_without_applying_or_hiding_the_run() {
        struct Malformed;
        impl ActionBackend for Malformed {
            fn invoke(
                &self,
                _agent: &AgentDefinition,
                _action: AgentAction,
                _input: &str,
                _model: Option<&str>,
                _effort: Option<ReasoningEffort>,
            ) -> Result<ActionExecution> {
                Ok(ActionExecution {
                    output: "not-json".into(),
                    token_usage: None,
                })
            }
        }
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("orc.db");
        let db = Database::init(&db_path).unwrap();
        let project = db.create_project("project").unwrap();
        db.insert_agent(&agent()).unwrap();
        let request: PlanningRequest = serde_json::from_value(serde_json::json!({
            "protocol_version": crate::protocol::PROTOCOL_VERSION,
            "kind": "project_plan",
            "project": null,
            "engineering_contract": "",
            "objective": "plan",
            "constraints": [], "target_platforms": [], "stack": [], "non_goals": [],
            "deliverables": [], "definition_of_done": [],
            "response_schema": crate::protocol::PlanResponseSchema::v1(),
            "role_boundaries": [], "planning_constraints": [], "approval_requirements": [],
            "current_state": null, "full_report": null
        }))
        .unwrap();
        assert!(run_plan(&db, &request, &ActionOverrides::default(), &Malformed).is_err());
        assert!(db.list_tasks().unwrap().is_empty());
        let run = &db.list_agent_runs(project, 10).unwrap()[0];
        assert_eq!(run.status, "failed");
        assert_eq!(run.output.as_deref(), Some("not-json"));
        assert!(run.error.as_deref().unwrap().contains("malformed"));
        drop(db);
        let reopened = Database::open(db_path).unwrap();
        let run = &reopened.list_agent_runs(project, 10).unwrap()[0];
        assert_eq!(run.output.as_deref(), Some("not-json"));
        assert!(run.error.as_deref().unwrap().contains("malformed"));
    }

    #[test]
    fn automated_action_persists_and_emits_provider_progress() {
        struct ProgressBackend {
            observed: RefCell<Vec<String>>,
        }
        impl ActionBackend for ProgressBackend {
            fn invoke(
                &self,
                _agent: &AgentDefinition,
                _action: AgentAction,
                _input: &str,
                _model: Option<&str>,
                _effort: Option<ReasoningEffort>,
            ) -> Result<ActionExecution> {
                unreachable!()
            }

            fn invoke_with_progress(
                &self,
                _agent: &AgentDefinition,
                _action: AgentAction,
                _input: &str,
                _model: Option<&str>,
                _effort: Option<ReasoningEffort>,
                progress: ActionProgress<'_>,
            ) -> Result<ActionExecution> {
                assert!(progress.schema.contains("protocol_version"));
                (progress.callback)("provider turn.started");
                (progress.callback)("provider item.completed: agent_message");
                Ok(ActionExecution {
                    output: serde_json::json!({
                        "protocol_version": crate::protocol::PROTOCOL_VERSION,
                        "objective": "proposed", "assumptions": [], "risks": [],
                        "questions": [], "tasks": [serde_json::json!({
                            "local_id": "progress-check", "title": "Progress check", "objective": "Verify progress", "role": "developer",
                            "priority": "normal", "depends_on": [], "capabilities": [], "scope_mode": null,
                            "context_files": ["README.md"], "expected_changes": ["README.md"], "unchanged": ["task state"],
                            "acceptance_criteria": ["progress is recorded"], "required_tests": ["cargo test"], "validation": ["cargo test"], "execution_hints": {"effort":"low","effort_reason":"isolated progress check"}, "risk_factors": []
                        })]
                    })
                    .to_string(),
                    token_usage: None,
                })
            }

            fn observe(&self, message: &str) {
                self.observed.borrow_mut().push(message.into());
            }
        }
        let directory = tempdir().unwrap();
        std::fs::create_dir(directory.path().join(".orc")).unwrap();
        std::fs::write(directory.path().join(".orc/engineering.md"), "contract").unwrap();
        let db = Database::init(directory.path().join("orc.db")).unwrap();
        let project = db.create_project("project").unwrap();
        db.insert_agent(&agent()).unwrap();
        let app =
            crate::app::OrcApp::open(directory.path().join("orc.db"), directory.path()).unwrap();
        let backend = ProgressBackend {
            observed: RefCell::new(Vec::new()),
        };
        let request = app.planning_request().unwrap();
        let run = app
            .automated_plan_with_backend(&request, &ActionOverrides::default(), &backend)
            .unwrap()
            .0;
        let stored = db.get_agent_run(run).unwrap().unwrap();
        assert_eq!(
            stored.phase.as_deref(),
            Some("provider item.completed: agent_message")
        );
        let events = db.list_lifecycle_events_for_run(run, 10).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.payload.as_deref() == Some("provider turn.started"))
        );
        let observed = backend.observed.borrow();
        assert!(observed[0].contains("Automated plan run"));
        assert!(observed[0].contains("agent=multi"));
        assert!(
            observed
                .iter()
                .any(|event| event == "provider turn.started")
        );
        assert_eq!(db.list_agent_runs(project, 10).unwrap().len(), 1);
    }
}
