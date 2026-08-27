use std::cell::RefCell;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::lead::{LeadBackend, LeadBackendResponse, LeadContext, LeadResponse, LeadService};
use crate::protocol::{PlanResponse, PlanningRequest};
use crate::registry::{self, AgentAction, AgentActionProfile, AgentDefinition, ReasoningEffort};
use crate::review::ReviewSummary;
use crate::storage::{AgentRunExecution, Database};
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
    let agents = db.list_agents()?;
    let agent = if let Some(id) = &overrides.agent_id {
        let agent = registry::get_agent(db, id)?;
        if agent.execution_mode != registry::AUTOMATED
            || !agent.is_selectable(&[])
            || !agent.supports_action(action)
        {
            bail!(
                "agent '{}' is unavailable or ineligible for '{}'",
                id,
                action.as_str()
            );
        }
        agent
    } else {
        registry::select_agent_for_action(&agents, action, &[])?.clone()
    };
    let profile = db
        .agent_action_profiles(&agent.id)?
        .into_iter()
        .find(|profile| profile.action == action)
        .unwrap_or(AgentActionProfile {
            action,
            model: None,
            reasoning_effort: None,
        });
    let resolved = ResolvedAction {
        action,
        agent: agent.id.clone(),
        model: overrides
            .model
            .clone()
            .or(profile.model)
            .or(agent.model.clone()),
        reasoning_effort: overrides
            .reasoning_effort
            .or(profile.reasoning_effort)
            .or(agent.reasoning_effort),
    };
    Ok((agent, resolved))
}

pub trait ActionBackend {
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
}

impl WorkerActionBackend {
    pub fn new(repo: impl AsRef<Path>) -> Self {
        Self {
            repo: repo.as_ref().to_path_buf(),
        }
    }
}

impl ActionBackend for WorkerActionBackend {
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
        let worker = if action == AgentAction::Lead {
            crate::backend::WorkerFactory::build_lead(agent, model.map(str::to_owned), effort)
        } else {
            crate::backend::WorkerFactory::build_with_codex_overrides(
                agent,
                model.map(str::to_owned),
                effort,
            )
        }
        .map_err(anyhow::Error::msg)?;
        let execution = worker
            .execute_structured_with_progress_and_usage(
                input,
                &self.repo,
                progress.schema,
                &|event| {
                    (progress.callback)(&provider_activity(event));
                },
            )
            .map_err(anyhow::Error::msg)?;
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
        eprintln!("{message}");
    }
}

fn provider_activity(event: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(event) else {
        return "provider activity".into();
    };
    let event_type = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("activity");
    let item_type = value
        .pointer("/item/type")
        .and_then(serde_json::Value::as_str);
    match item_type {
        Some(item_type) => format!("provider {event_type}: {item_type}"),
        None => format!("provider {event_type}"),
    }
}

fn schema(action: AgentAction) -> String {
    let string_array = serde_json::json!({"type":"array","items":{"type":"string"}});
    let planned_task = serde_json::json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "local_id":{"type":"string"},"title":{"type":"string"},"objective":{"type":"string"},
            "role":{"type":"string"},"priority":{"enum":["low","normal","high","critical"]},
            "depends_on":string_array,"capabilities":string_array,
            "scope_mode":{"type":["string","null"]},"context_files":string_array,"expected_changes":string_array
        },
        "required":["local_id","title","objective","role","priority","depends_on","capabilities","scope_mode","context_files","expected_changes"]
    });
    let plan = serde_json::json!({
        "type":"object","additionalProperties":false,
        "properties":{"protocol_version":{"type":"integer"},"objective":{"type":"string"},"assumptions":string_array,"risks":string_array,"questions":string_array,"tasks":{"type":"array","items":planned_task}},
        "required":["protocol_version","objective","assumptions","risks","questions","tasks"]
    });
    let value = match action {
        AgentAction::Review => serde_json::json!({
            "type":"object","additionalProperties":false,
            "properties":{"verdict":{"type":"string"},"findings":string_array,"blocking_findings":string_array,"non_blocking_findings":string_array,"severity":{"type":["string","null"]},"revision_feedback":{"type":["string","null"]},"blockers":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"id":{"type":"string"},"prior_blocker_id":{"type":["string","null"]},"blocker_key":{"type":"string","minLength":1},"requirement_ref":{"type":"string"},"evidence":{"type":"string"},"severity":{"type":"string","enum":["low","medium","high","critical","unspecified"]},"acceptance_condition":{"type":"string"},"status":{"type":"string","enum":["new","unresolved","resolved","regression"]},"finding":{"type":"string"}},"required":["id","prior_blocker_id","blocker_key","requirement_ref","evidence","severity","acceptance_condition","status","finding"]}}},
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
            ]}}},
            "required":["message","proposals"]
        }),
        AgentAction::Code => serde_json::json!({"type":"object"}),
    };
    value.to_string()
}

/// Native provider schema for the structured result returned by a revision worker.
pub fn revision_handoff_schema() -> String {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
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
                        "evidence": {"type": "array", "items": {
                            "type": "object", "additionalProperties": false,
                            "properties": {
                                "changed_file": {"type": "string"},
                                "validation_command": {"type": "string"},
                                "test_names": {"type": "array", "items": {"type": "string"}}
                            },
                            "required": ["changed_file", "validation_command", "test_names"]
                        }},
                        "validation_evidence": {"type": "string"},
                        "unresolved_risk": {"type": ["string", "null"]}
                    },
                    "required": [
                        "blocker_id",
                        "status",
                        "implementation_summary",
                        "changed_files",
                        "evidence",
                        "validation_evidence",
                        "unresolved_risk"
                    ]
                }
            }
        },
        "required": ["claims"]
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevisionContract {
    pub unresolved: Vec<crate::storage::db::ReviewBlockerRecord>,
    pub regressions: Vec<crate::storage::db::ReviewBlockerRecord>,
    pub regression_constraints: Vec<crate::storage::db::ReviewBlockerRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionClaim {
    pub blocker_id: String,
    pub status: String,
    pub implementation_summary: String,
    pub changed_files: Vec<String>,
    pub evidence: Vec<RevisionClaimEvidence>,
    pub validation_evidence: String,
    pub unresolved_risk: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RevisionClaimEvidence {
    pub changed_file: String,
    pub validation_command: String,
    #[serde(default)]
    pub test_names: Vec<String>,
}

/// Validation captured after the current diff was inspected. The fingerprint
/// prevents an otherwise valid report from being replayed after the worktree changes.
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
    pub claims: Vec<RevisionClaim>,
}

pub fn validate_revision_handoff(
    contract: &RevisionContract,
    output: &str,
) -> Result<RevisionHandoff> {
    validate_revision_handoff_with_evidence(contract, output, None, None)
}

/// Validate a revision handoff against evidence captured for this revision.
/// The optional arguments retain compatibility for callers which only need the
/// legacy empty-contract behavior; active blocker claims require both records.
pub fn validate_revision_handoff_with_evidence(
    contract: &RevisionContract,
    output: &str,
    changes: Option<&crate::git::WorktreeChanges>,
    validation_payload: Option<&str>,
) -> Result<RevisionHandoff> {
    let active_count = contract.unresolved.len() + contract.regressions.len();
    // Preserve the legacy one-shot revision path when the authoritative ledger
    // contains no active blocker work. There is no claim to validate in that case.
    if active_count == 0 {
        return Ok(RevisionHandoff { claims: Vec::new() });
    }
    let handoff: RevisionHandoff = serde_json::from_str(output)
        .context("revision worker did not return a structured handoff")?;
    let required: std::collections::BTreeSet<_> = contract
        .unresolved
        .iter()
        .chain(contract.regressions.iter())
        .map(|b| b.blocker_id.as_str())
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for claim in &handoff.claims {
        if !matches!(claim.status.as_str(), "addressed" | "unresolved") {
            bail!(
                "revision handoff claim '{}' has invalid status",
                claim.blocker_id
            );
        }
        contract
            .unresolved
            .iter()
            .chain(contract.regressions.iter())
            .find(|b| b.blocker_id == claim.blocker_id)
            .ok_or_else(|| anyhow::anyhow!("unknown blocker ID '{}'", claim.blocker_id))?;
        if claim.implementation_summary.trim().is_empty() {
            bail!(
                "revision handoff claim '{}' is missing implementation or validation evidence",
                claim.blocker_id
            );
        }
        if claim.validation_evidence.trim().is_empty() {
            bail!(
                "revision handoff claim '{}' is missing implementation or validation evidence",
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
        if is_vacuous_text(&claim.validation_evidence) {
            bail!(
                "revision handoff claim '{}' contains placeholder validation evidence",
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
        let validation = validation_payload
            .context("active revision claims require current validation evidence")?;
        let validation: RevisionValidationEvidence = serde_json::from_str(validation)
            .context("current revision validation evidence is not structured")?;
        if validation.worktree_fingerprint != revision_worktree_fingerprint(changes) {
            bail!(
                "revision handoff claim '{}' is supported by stale validation evidence",
                claim.blocker_id
            );
        }
        let report = &validation.report;
        if !report.is_success() {
            bail!(
                "revision handoff claim '{}' is supported by failed validation",
                claim.blocker_id
            );
        }
        let evidence_paths: std::collections::BTreeSet<_> = claim
            .evidence
            .iter()
            .map(|evidence| evidence.changed_file.as_str())
            .collect();
        let claimed_paths: std::collections::BTreeSet<_> =
            claim.changed_files.iter().map(String::as_str).collect();
        if evidence_paths != claimed_paths {
            bail!(
                "revision handoff claim '{}' is not tied to its changed files or acceptance condition",
                claim.blocker_id
            );
        }
        for evidence in &claim.evidence {
            let step = report
                .steps
                .iter()
                .find(|step| step.command == evidence.validation_command && step.passed)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "revision handoff claim '{}' lacks fresh passing evidence for command '{}'",
                        claim.blocker_id,
                        evidence.validation_command
                    )
                })?;
            let patch = changed_file_patch(changes, &evidence.changed_file);
            for test_name in &evidence.test_names {
                if test_name.trim().is_empty()
                    || !patch.contains(test_name)
                    || !step.output().contains(test_name)
                {
                    bail!(
                        "revision handoff claim '{}' contains fabricated or unexecuted test name '{}'",
                        claim.blocker_id,
                        test_name
                    );
                }
            }
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

fn changed_file_patch<'a>(changes: &'a crate::git::WorktreeChanges, path: &str) -> &'a str {
    let marker_a = format!("a/{path}");
    let marker_b = format!("b/{path}");
    changes
        .diff
        .split("diff --git ")
        .skip(1)
        .find(|section| {
            section
                .lines()
                .next()
                .is_some_and(|header| header.contains(&marker_a) || header.contains(&marker_b))
        })
        .unwrap_or_default()
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
    RevisionContract {
        unresolved,
        regressions,
        regression_constraints,
    }
}

pub fn format_revision_contract(contract: &RevisionContract) -> String {
    let mut out = String::from("## Revision contract\n\n");
    out.push_str("### Unresolved blockers (implement and prove each)\n");
    if contract.unresolved.is_empty() {
        out.push_str("- None recorded; verify the supplied review feedback.\n");
    }
    for blocker in &contract.unresolved {
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
    if contract.regression_constraints.is_empty() {
        out.push_str("- None recorded.\n");
    }
    for blocker in &contract.regression_constraints {
        out.push_str(&format!("- {} | acceptance: {} | preserve the resolved behavior unless current evidence proves regression\n", blocker.blocker_id, blocker.acceptance_condition));
    }
    out.push_str("### Regressions (implement and prove each)\n");
    for blocker in &contract.regressions {
        out.push_str(&format!(
            "- {} | acceptance: {} | finding: {}\n",
            blocker.blocker_id, blocker.acceptance_condition, blocker.finding
        ));
    }
    out.push_str("\n### Required handoff\nReturn JSON {\"claims\":[{\"blocker_id\":\"...\",\"status\":\"addressed|unresolved\",\"implementation_summary\":\"...\",\"changed_files\":[],\"validation_evidence\":\"...\",\"unresolved_risk\":null}]} with exactly one claim for every active blocker ID. Resolved constraints require no claim. Keep changes focused.");
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

const TASK_REVIEW_INSTRUCTIONS: &str = "Perform an acceptance-first, task-scoped contract review. Use the task contract, submitted diff, persisted structured validation evidence, and review history; worker narrative is not validation evidence. On a revision, verify every unresolved blocker against the current implementation and fresh evidence before considering a broad review. Check each resolved blocker for regression; Equivalent or reworded findings refer to the same concern and remain resolved. Reopen a resolved concern only when current implementation evidence demonstrates a genuine regression, and explain that evidence. Clearly distinguish RESOLVED from UNRESOLVED prior blockers in your findings. Do not restate equivalent findings. Reject vacuous or placeholder tests, assertions, changed-file lists, and validation claims: a test must exercise the production behavior and an assertion must observe its outcome, not merely name a requirement or duplicate a constant. A blocker must identify an explicit requirement, concrete current evidence, and why acceptance is prevented; only unmet requirements, incorrect required workflow, material regressions, safety/data-integrity failures, or failed/materially absent structured validation can block. If required commands are outside the persisted project validation pipeline, report that accurately rather than requesting fabricated evidence. Keep blocking findings to at most 5. PASS requires no blocking findings; REVISE requires focused in-scope changes; REJECT is only for fundamental contradiction or unsafe implementation. Escalate to a full review only after blocker verification passes, or when the architecture changed materially.";

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
            source: "action",
        },
    )?)
}

fn announce_run(backend: &dyn ActionBackend, run: i64, resolved: &ResolvedAction) {
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
    db.update_agent_run_phase(run, "provider starting")?;
    backend.observe("provider starting");
    let progress = |activity: &str| {
        if let Err(error) = db.update_agent_run_phase(run, activity) {
            backend.observe(&format!(
                "warning: failed to persist action progress: {error}"
            ));
        }
        backend.observe(activity);
    };
    let action_schema = schema(resolved.action);
    backend.invoke_with_progress(
        agent,
        resolved.action,
        prompt,
        resolved.model.as_deref(),
        resolved.reasoning_effort,
        ActionProgress {
            schema: &action_schema,
            callback: &progress,
        },
    )
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

pub fn run_review(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
) -> Result<(i64, ReviewResult)> {
    run_review_mode(db, summary, overrides, backend, false)
}

pub fn run_project_review(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
) -> Result<(i64, ReviewResult)> {
    run_review_mode(db, summary, overrides, backend, true)
}

fn run_review_mode(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
    project_review: bool,
) -> Result<(i64, ReviewResult)> {
    let (agent, resolved) = resolve_action(db, AgentAction::Review, overrides)?;
    let run = db.create_project_action_run(
        db.get_project_id()?.context("no project found in DB")?,
        (!project_review).then_some(summary.task.id.as_str()),
        AgentAction::Review.as_str(),
        &resolved.agent,
        AgentRunExecution {
            class: AgentAction::Review.as_str(),
            model: resolved.model.as_deref(),
            effort: resolved.reasoning_effort,
            source: "action",
        },
    )?;
    let _run_finalizer = db.run_finalizer(run);
    let instructions = if project_review {
        "Perform a project-wide audit. Inspect broader architecture, latent defects, consistency, technical debt, missing tests, and adjacent concerns without task-scope restrictions. Classify findings in blocking_findings or non_blocking_findings for this project audit."
    } else {
        TASK_REVIEW_INSTRUCTIONS
    };
    let history = if project_review {
        String::new()
    } else {
        review_resolution_ledger(&summary.prior_reviews)
    };
    let prompt = format!(
        "{instructions} Return only JSON matching {{\"verdict\":string,\"findings\":[string],\"blocking_findings\":[string],\"non_blocking_findings\":[string],\"severity\":string|null,\"revision_feedback\":string|null,\"blockers\":[{{\"id\":string,\"prior_blocker_id\":string|null,\"blocker_key\":string,\"requirement_ref\":string,\"evidence\":string,\"severity\":string,\"acceptance_condition\":string,\"status\":\"new|unresolved|resolved|regression\",\"finding\":string}}]}}. blocker_key is required for readability but is not identity. Reference an existing blocker_id as prior_blocker_id for the same underlying issue; use null only for genuinely new blockers. Copy every prior_blocker_id verbatim from the ledger, including every hexadecimal character; never shorten, regenerate, or retype an ID from memory. Do not accept or merge the task.\nCurrent blocker ledger (IDs are authoritative):\n{history}\nReview packet:\n{}",
        serde_json::to_string(summary)?
    );
    let execution = invoke_action(db, run, backend, &agent, &resolved, &prompt);
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
                            blocker.id = canonical_id;
                            blocker.status = if old.status == "resolved" {
                                "regression"
                            } else {
                                "unresolved"
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
                    let persisted_output = serde_json::to_string(&result)?;
                    if !project_review {
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
                            };
                            for record in records {
                                match record.status.as_str() {
                                    "resolved" => contract.regression_constraints.push(record),
                                    "regression" => contract.regressions.push(record),
                                    _ => contract.unresolved.push(record),
                                }
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
    let (agent, resolved) = resolve_action(db, AgentAction::Plan, overrides)?;
    let run = start_run(db, AgentAction::Plan, &resolved)?;
    let _run_finalizer = db.run_finalizer(run);
    let prompt = format!(
        "Produce a plan for this request. Return only a PlanResponse JSON document and do not mutate project state.\n{}",
        serde_json::to_string(request)?
    );
    let execution = invoke_action(db, run, backend, &agent, &resolved, &prompt);
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
        let input =
            serde_json::to_string(&(context, message)).map_err(|error| error.to_string())?;
        let prompt = format!(
            "Act as Orc's project Lead. Return only JSON matching {{\"message\":string,\"proposals\":array}}. Proposals are human-gated and must not be applied.\n{input}"
        );
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
        let response: LeadBackendResponse =
            serde_json::from_str(&execution.output).map_err(|error| {
                format!("Lead provider returned malformed structured output: {error}")
            })?;
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
    let (agent, resolved) = resolve_action(db, AgentAction::Lead, overrides)?;
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
    use crate::registry::{AUTOMATED, AVAILABLE};
    use crate::task::TaskPriority;
    use tempfile::tempdir;

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
        }
    }

    #[test]
    fn addressed_handoff_rejects_vacuous_claim_without_changed_files() {
        let output = serde_json::json!({"claims": [{
            "blocker_id": "BLK-1", "status": "addressed",
            "implementation_summary": "fixed it", "changed_files": [],
            "evidence": [],
            "validation_evidence": "cargo test passed", "unresolved_risk": null
        }]})
        .to_string();
        let error = validate_revision_handoff(&handoff_contract(), &output).unwrap_err();
        assert!(error.to_string().contains("changed files"));
    }

    #[test]
    fn handoff_rejects_placeholder_validation_evidence() {
        let output = serde_json::json!({"claims": [{
            "blocker_id": "BLK-1", "status": "addressed",
            "implementation_summary": "fixed it", "changed_files": ["src/lib.rs"],
            "evidence": [{"changed_file":"src/lib.rs","validation_command":"cargo test","test_names":[]}],
            "validation_evidence": "not tested", "unresolved_risk": null
        }]})
        .to_string();
        let error = validate_revision_handoff(&handoff_contract(), &output).unwrap_err();
        assert!(error.to_string().contains("placeholder validation"));
    }

    fn test_evidence_fixture(
        changed_path: &str,
        test_in_patch: &str,
        executed_test: &str,
    ) -> (crate::git::WorktreeChanges, String, String) {
        let changes = crate::git::WorktreeChanges {
            files: vec![crate::git::ChangedFile {
                status: "M".into(),
                path: changed_path.into(),
            }],
            stat: format!("{changed_path} | 1 +"),
            diff: format!(
                "diff --git a/{changed_path} b/{changed_path}\n--- a/{changed_path}\n+++ b/{changed_path}\n+fn {test_in_patch}() {{}}\n"
            ),
        };
        let validation = RevisionValidationEvidence {
            evidence_id: "validation-current".into(),
            worktree_fingerprint: revision_worktree_fingerprint(&changes),
            report: crate::validation::ValidationReport {
                steps: vec![crate::validation::ValidationStepResult {
                    command: "cargo test persisted_revision_contract_lifecycle".into(),
                    category: crate::validation::ValidationCategory::Test,
                    passed: true,
                    stdout: format!("test {executed_test} ... ok"),
                    stderr: String::new(),
                    exit_status: Some(0),
                    diagnostics: None,
                    failure_classification: None,
                    fallback_command: None,
                }],
            },
        };
        let handoff = serde_json::json!({"claims": [{
            "blocker_id": "BLK-1", "status": "addressed",
            "implementation_summary": "Added deterministic lifecycle coverage.",
            "changed_files": [changed_path],
            "evidence": [{
                "changed_file": changed_path,
                "validation_command": "cargo test persisted_revision_contract_lifecycle",
                "test_names": ["persisted_revision_contract_lifecycle"]
            }],
            "validation_evidence": "Named test evidence is attached.",
            "unresolved_risk": null
        }]})
        .to_string();
        (
            changes,
            serde_json::to_string(&validation).unwrap(),
            handoff,
        )
    }

    #[test]
    fn changed_test_without_fresh_test_evidence_is_rejected() {
        let (changes, _, handoff) = test_evidence_fixture(
            "tests/lifecycle.rs",
            "persisted_revision_contract_lifecycle",
            "persisted_revision_contract_lifecycle",
        );
        let error = validate_revision_handoff_with_evidence(
            &handoff_contract(),
            &handoff,
            Some(&changes),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("current validation evidence"));
    }

    #[test]
    fn fresh_validation_with_unchanged_claimed_file_is_rejected() {
        let (changes, validation, handoff) = test_evidence_fixture(
            "tests/other.rs",
            "persisted_revision_contract_lifecycle",
            "persisted_revision_contract_lifecycle",
        );
        let handoff = handoff.replace("tests/other.rs", "tests/lifecycle.rs");
        let error = validate_revision_handoff_with_evidence(
            &handoff_contract(),
            &handoff,
            Some(&changes),
            Some(&validation),
        )
        .unwrap_err();
        assert!(error.to_string().contains("file not changed"));
    }

    #[test]
    fn fabricated_test_name_is_rejected() {
        let (changes, validation, handoff) = test_evidence_fixture(
            "tests/lifecycle.rs",
            "persisted_revision_contract_lifecycle",
            "some_other_test",
        );
        let error = validate_revision_handoff_with_evidence(
            &handoff_contract(),
            &handoff,
            Some(&changes),
            Some(&validation),
        )
        .unwrap_err();
        assert!(error.to_string().contains("fabricated or unexecuted"));
    }

    #[test]
    fn validation_from_previous_fingerprint_is_rejected() {
        let (changes, validation, handoff) = test_evidence_fixture(
            "tests/lifecycle.rs",
            "persisted_revision_contract_lifecycle",
            "persisted_revision_contract_lifecycle",
        );
        let mut validation: RevisionValidationEvidence = serde_json::from_str(&validation).unwrap();
        validation.worktree_fingerprint = "rev-previous".into();
        let error = validate_revision_handoff_with_evidence(
            &handoff_contract(),
            &handoff,
            Some(&changes),
            Some(&serde_json::to_string(&validation).unwrap()),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stale validation"));
    }

    #[test]
    fn unrelated_changed_file_is_rejected_even_when_named_test_ran() {
        let (changes, validation, handoff) = test_evidence_fixture(
            "tests/unrelated.rs",
            "unrelated_test",
            "persisted_revision_contract_lifecycle",
        );
        let error = validate_revision_handoff_with_evidence(
            &handoff_contract(),
            &handoff,
            Some(&changes),
            Some(&validation),
        )
        .unwrap_err();
        assert!(error.to_string().contains("fabricated or unexecuted"));
    }

    #[test]
    fn current_diff_matching_test_and_fresh_passing_evidence_is_accepted() {
        let (changes, validation, handoff) = test_evidence_fixture(
            "tests/lifecycle.rs",
            "persisted_revision_contract_lifecycle",
            "persisted_revision_contract_lifecycle",
        );
        validate_revision_handoff_with_evidence(
            &handoff_contract(),
            &handoff,
            Some(&changes),
            Some(&validation),
        )
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
            capabilities: Vec::new(),
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
            .automated_review_with_backend(&task, &ActionOverrides::default(), &backend)
            .unwrap()
            .1;
        assert_eq!(review.verdict, "revise");
        assert_eq!(
            app.task(&task).unwrap().unwrap().status.to_string(),
            "backlog"
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
        let instructions = "Equivalent or reworded findings refer to the same concern and remain resolved. Reopen a resolved concern only when current implementation evidence demonstrates a genuine regression";

        assert!(TASK_REVIEW_INSTRUCTIONS.contains(instructions));
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
        let result = run_review(&db, &summary, &ActionOverrides::default(), &backend)
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
            let result = run_review(&db, &summary, &ActionOverrides::default(), &backend)
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
        let (run_id, _) = run_review(&db, &summary, &ActionOverrides::default(), &backend).unwrap();
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
        run_review(&db, &summary, &ActionOverrides::default(), &backend).unwrap();

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
                        "questions": [], "tasks": []
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
