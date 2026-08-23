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
        _action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<ActionExecution> {
        let worker = crate::backend::WorkerFactory::build_with_codex_overrides(
            agent,
            model.map(str::to_owned),
            effort,
        )
        .map_err(anyhow::Error::msg)?;
        let execution = worker
            .execute_with_progress_and_usage(input, &self.repo, &|_| {})
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewResult {
    pub verdict: String,
    #[serde(default)]
    pub findings: Vec<String>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub revision_feedback: Option<String>,
}

fn start_run(db: &Database, action: AgentAction, resolved: &ResolvedAction) -> Result<i64> {
    let project_id = db.get_project_id()?.context("no project found in DB")?;
    Ok(db.create_project_action_run(
        project_id,
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

fn fail_run(
    db: &Database,
    run: i64,
    error: &anyhow::Error,
    usage: Option<TokenUsage>,
) -> Result<()> {
    db.update_agent_run_status_with_usage(run, "failed", Some(&error.to_string()), usage)?;
    Ok(())
}

pub fn run_review(
    db: &Database,
    summary: &ReviewSummary,
    overrides: &ActionOverrides,
    backend: &dyn ActionBackend,
) -> Result<(i64, ReviewResult)> {
    let (agent, resolved) = resolve_action(db, AgentAction::Review, overrides)?;
    let run = start_run(db, AgentAction::Review, &resolved)?;
    let prompt = format!(
        "Review this completed task. Return only JSON matching {{\"verdict\":string,\"findings\":[string],\"severity\":string|null,\"revision_feedback\":string|null}}. Do not accept or merge the task.\n{}",
        serde_json::to_string(summary)?
    );
    let execution = backend.invoke(
        &agent,
        AgentAction::Review,
        &prompt,
        resolved.model.as_deref(),
        resolved.reasoning_effort,
    );
    match execution {
        Ok(execution) => {
            let parsed = serde_json::from_str::<ReviewResult>(&execution.output)
                .context("reviewer returned malformed structured output")
                .and_then(|result| {
                    if result.verdict.trim().is_empty() {
                        bail!("review verdict must not be empty")
                    }
                    Ok(result)
                });
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
                    fail_run(db, run, &error, execution.token_usage)?;
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
    let execution = backend.invoke(
        &agent,
        AgentAction::Plan,
        &prompt,
        resolved.model.as_deref(),
        resolved.reasoning_effort,
    );
    match execution {
        Ok(execution) => {
            let parsed = serde_json::from_str::<PlanResponse>(&execution.output)
                .context("planner returned malformed structured output")
                .and_then(|plan| {
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
                    fail_run(db, run, &error, execution.token_usage)?;
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
        let execution = self
            .backend
            .invoke(
                self.agent,
                AgentAction::Lead,
                &prompt,
                self.resolved.model.as_deref(),
                self.resolved.reasoning_effort,
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
            fail_run(db, run, &error, *adapter.usage.borrow())?;
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
                output: output.to_string(),
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
        let db = Database::init(directory.path().join("orc.db")).unwrap();
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
        assert_eq!(db.list_agent_runs(project, 10).unwrap()[0].status, "failed");
    }
}
