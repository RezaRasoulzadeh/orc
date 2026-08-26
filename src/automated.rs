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
        _action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
        progress: ActionProgress<'_>,
    ) -> Result<ActionExecution> {
        let worker = crate::backend::WorkerFactory::build_with_codex_overrides(
            agent,
            model.map(str::to_owned),
            effort,
        )
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
            "properties":{"verdict":{"type":"string"},"findings":string_array,"blocking_findings":string_array,"non_blocking_findings":string_array,"severity":{"type":["string","null"]},"revision_feedback":{"type":["string","null"]}},
            "required":["verdict","findings","blocking_findings","non_blocking_findings","severity","revision_feedback"]
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
}

fn review_resolution_ledger(reviews: &[crate::review::PriorReview]) -> String {
    if reviews.is_empty() {
        return "No prior task reviews.".into();
    }
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    for review in reviews {
        if review.verdict.eq_ignore_ascii_case("pass") {
            resolved.append(&mut unresolved);
        } else {
            unresolved.extend(review.blocking_findings.iter().cloned());
        }
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

const TASK_REVIEW_INSTRUCTIONS: &str = "Perform an acceptance-first, task-scoped contract review. Use the task contract, submitted diff, persisted structured validation evidence, and review history; worker narrative is not validation evidence. On a revision, evaluate prior blockers first using the resolution ledger: a blocker from a prior review whose later review classified it resolved, or whose later PASS had no blocking findings, is RESOLVED and must not be reported as blocking again. Equivalent or reworded findings refer to the same concern and remain resolved. Reopen a resolved concern only when current implementation evidence demonstrates a genuine regression, and explain that evidence. Clearly distinguish RESOLVED from UNRESOLVED prior blockers in your findings. Do not restate equivalent findings. A blocker must identify an explicit requirement, concrete current evidence, and why acceptance is prevented; only unmet requirements, incorrect required workflow, material regressions, safety/data-integrity failures, or failed/materially absent structured validation can block. If required commands are outside the persisted project validation pipeline, report that accurately rather than requesting fabricated evidence. Keep blocking findings to at most 5. Code quality, polish, preferences, extra tests, and unrelated defects are non-blocking. PASS requires no blocking findings; REVISE requires focused in-scope changes; REJECT is only for fundamental contradiction or unsafe implementation.";

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
        "{instructions} Return only JSON matching {{\"verdict\":string,\"findings\":[string],\"blocking_findings\":[string],\"non_blocking_findings\":[string],\"severity\":string|null,\"revision_feedback\":string|null}}. Do not accept or merge the task.\nResolution ledger:\n{history}\nReview packet:\n{}",
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
                    Ok(result)
                },
            );
            match parsed {
                Ok(result) => {
                    db.update_agent_run_status_with_usage(
                        run,
                        "completed",
                        Some(&execution.output),
                        execution.token_usage,
                    )?;
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
        serde_json::from_str(&execution.output)
            .map_err(|error| format!("Lead provider returned malformed structured output: {error}"))
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
    let adapter = LeadActionAdapter {
        db,
        run,
        backend,
        agent: &agent,
        resolved: &resolved,
        usage: RefCell::new(None),
        output: RefCell::new(None),
    };
    match LeadService::new(db, repo).invoke(message, &adapter, 50) {
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
                    }]
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
