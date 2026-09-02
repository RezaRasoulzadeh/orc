//! Provider-independent system acceptance tests.
//!
//! These tests deliberately use production lifecycle, scheduler, persistence,
//! validation, review, and operations APIs. Only provider/command transports
//! are scripted.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anyhow::Result;
use orc::agent::{
    dispatch_manual, dispatch_with_worker_on_db, revise_with_worker_on_db,
    submit_patch_with_runner, submit_run_with_runner,
};
use orc::app::OrcApp;
use orc::automated::{ActionBackend, ActionExecution, ActionOverrides, blocker_id};
use orc::operations::{OperationalNextStep, ProjectOperations, ValidationState};
use orc::registry::{
    AUTOMATED, AVAILABLE, AgentAction, AgentDefinition, EconomyCostConfiguration, EconomyTier,
    MANUAL, ReasoningEffort,
};
use orc::scheduler::{
    EconomyOverrides, QuotaRefresher, TransportEligibility,
    resolve_task_economy_for_execution_with_refresher,
};
use orc::storage::{AgentRunExecution, Database};
use orc::task::{TaskPriority, TaskStatus};
use orc::validation::{ValidationCategory, ValidationRunner, ValidationStepResult};
use orc::worker::{TokenUsage, Worker, WorkerExecution, WorkerOutcome};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn agent(
    id: &str,
    mode: &str,
    model: Option<&str>,
    priority: i64,
    actions: Vec<AgentAction>,
) -> AgentDefinition {
    AgentDefinition {
        id: id.into(),
        backend: "acceptance-fake".into(),
        execution_mode: mode.into(),
        display_name: id.into(),
        enabled: true,
        priority,
        capabilities: vec!["code".into(), "terminal".into(), "review".into()],
        status: AVAILABLE.into(),
        unavailable_reason: None,
        profile_path: None,
        model: model.map(str::to_owned),
        reasoning_effort: Some(ReasoningEffort::Low).filter(|_| mode == AUTOMATED),
        config_metadata: None,
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
        actions,
    }
}

struct TestProject {
    directory: TempDir,
    db: Database,
    task: String,
}

impl TestProject {
    fn new(validation_commands: &[&str]) -> Self {
        let directory = tempfile::tempdir().expect("temp repository");
        git(directory.path(), &["init", "."]);
        git(
            directory.path(),
            &["config", "user.email", "acceptance@example.com"],
        );
        git(directory.path(), &["config", "user.name", "Orc Acceptance"]);
        std::fs::create_dir_all(directory.path().join(".orc")).unwrap();
        std::fs::write(
            directory.path().join(".orc/engineering.md"),
            "# Acceptance contract\n\nReview is semantic only.\n",
        )
        .unwrap();
        let commands = validation_commands
            .iter()
            .map(|command| format!("\"{command}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            directory.path().join(".orc/validation.toml"),
            format!("commands = [{commands}]\n"),
        )
        .unwrap();
        std::fs::write(directory.path().join("README.md"), "base\n").unwrap();
        git(directory.path(), &["add", "."]);
        git(directory.path(), &["commit", "-m", "base"]);

        let db = Database::init(directory.path().join(".orc/orc.db")).unwrap();
        let project = db.create_project("acceptance").unwrap();
        db.insert_agent(&agent(
            "system-agent",
            AUTOMATED,
            Some("model-economy"),
            100,
            vec![AgentAction::Code, AgentAction::Review],
        ))
        .unwrap();
        // Queue eligibility is intentionally evaluated before the injected
        // Worker seam's narrow UnsupportedBackend exception. Keep one fully
        // production-supported registry attachment so the fake transport
        // cannot make an otherwise unschedulable task dispatchable.
        let mut eligibility_anchor = agent(
            "eligibility-anchor",
            MANUAL,
            None,
            1,
            vec![AgentAction::Code],
        );
        eligibility_anchor.backend = "generic_manual".into();
        db.insert_agent(&eligibility_anchor).unwrap();
        let task = db
            .insert_task(
                project,
                "System lifecycle",
                "implement the acceptance feature",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap();
        Self {
            directory,
            db,
            task,
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn app(&self) -> OrcApp {
        OrcApp::open(self.path().join(".orc/orc.db"), self.path()).unwrap()
    }

    fn worktree(&self, task: &str) -> PathBuf {
        let (_, path) = self.db.get_worktree_metadata(task).unwrap().unwrap();
        self.path().join(path)
    }

    fn add_task(&self, title: &str) -> String {
        self.db
            .insert_task(
                self.db.get_project_id().unwrap().unwrap(),
                title,
                "implement another acceptance feature",
                "developer",
                TaskPriority::Normal,
            )
            .unwrap()
    }
}

#[derive(Clone)]
struct WorkerTurn {
    file: &'static str,
    contents: &'static str,
    usage: Option<TokenUsage>,
}

struct ScriptedWorker {
    turns: Mutex<VecDeque<WorkerTurn>>,
    prompts: Mutex<Vec<String>>,
}

impl ScriptedWorker {
    fn new(turns: impl IntoIterator<Item = WorkerTurn>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            prompts: Mutex::new(Vec::new()),
        }
    }

    fn perform(&self, prompt: &str, cwd: &Path) -> Result<WorkerExecution, String> {
        self.prompts.lock().unwrap().push(prompt.to_owned());
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "scripted Worker exhausted".to_owned())?;
        std::fs::write(cwd.join(turn.file), turn.contents).map_err(|error| error.to_string())?;
        Ok(WorkerExecution {
            outcome: WorkerOutcome::Success,
            output: Some("scripted provider completion".into()),
            token_usage: turn.usage,
        })
    }
}

impl Worker for ScriptedWorker {
    fn execute(&self, prompt: &str, cwd: &Path) -> Result<(WorkerOutcome, Option<String>), String> {
        let result = self.perform(prompt, cwd)?;
        Ok((result.outcome, result.output))
    }

    fn execute_structured_with_progress_and_usage(
        &self,
        prompt: &str,
        cwd: &Path,
        _: &str,
        _: &dyn Fn(&str),
    ) -> Result<WorkerExecution, String> {
        let mut execution = self.perform(prompt, cwd)?;
        if let Some(packet) = prompt
            .split("## Authoritative Orc packet")
            .nth(1)
            .and_then(|value| value.find('{').map(|index| &value[index..]))
        {
            let packet: serde_json::Value =
                serde_json::from_str(packet).map_err(|error| error.to_string())?;
            let Some(plan) = packet.get("execution_plan") else {
                return Ok(execution);
            };
            let plan: orc::worker_protocol::WorkerPlan =
                serde_json::from_value(plan.clone()).map_err(|error| error.to_string())?;
            let step_results = plan
                .steps
                .iter()
                .map(|step| {
                    serde_json::json!({
                        "step_id": step.id,
                        "operations_performed": step.operations,
                        "affected_files": step.operation_targets,
                        "observed": ["scripted checkpoint completed"],
                        "verification_passed": []
                    })
                })
                .collect::<Vec<_>>();
            execution.output = Some(
                serde_json::json!({"step_results": step_results, "summary": "complete"})
                    .to_string(),
            );
        }
        Ok(execution)
    }
}

struct SequenceValidationRunner {
    results: Mutex<VecDeque<Result<ValidationStepResult, String>>>,
    commands: Mutex<Vec<String>>,
}

impl SequenceValidationRunner {
    fn passing(count: usize) -> Self {
        Self::new((0..count).map(|_| Ok(validation_step(true))))
    }

    fn new(results: impl IntoIterator<Item = Result<ValidationStepResult, String>>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().collect()),
            commands: Mutex::new(Vec::new()),
        }
    }
}

impl ValidationRunner for SequenceValidationRunner {
    fn run(&self, command: &str, _: &Path) -> Result<ValidationStepResult> {
        self.commands.lock().unwrap().push(command.into());
        self.results
            .lock()
            .unwrap()
            .pop_front()
            .expect("validation script exhausted")
            .map(|mut step| {
                step.command = command.into();
                step
            })
            .map_err(anyhow::Error::msg)
    }
}

fn validation_step(passed: bool) -> ValidationStepResult {
    ValidationStepResult {
        command: String::new(),
        category: ValidationCategory::Test,
        passed,
        stdout: if passed { "ok\n".into() } else { String::new() },
        stderr: if passed {
            String::new()
        } else {
            "current deterministic failure\n".into()
        },
        exit_status: Some(if passed { 0 } else { 1 }),
        diagnostics: None,
        failure_classification: None,
        fallback_command: None,
    }
}

type BackendCall = (AgentAction, String, Option<String>, Option<ReasoningEffort>);

struct ScriptedBackend {
    outputs: Mutex<VecDeque<(String, Option<TokenUsage>)>>,
    calls: Mutex<Vec<BackendCall>>,
}

impl ScriptedBackend {
    fn new(outputs: impl IntoIterator<Item = (String, Option<TokenUsage>)>) -> Self {
        Self {
            outputs: Mutex::new(outputs.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl ActionBackend for ScriptedBackend {
    fn invoke(
        &self,
        _: &AgentDefinition,
        action: AgentAction,
        input: &str,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Result<ActionExecution> {
        self.calls
            .lock()
            .unwrap()
            .push((action, input.into(), model.map(str::to_owned), effort));
        let (output, token_usage) = self
            .outputs
            .lock()
            .unwrap()
            .pop_front()
            .expect("backend script exhausted");
        let output = if action == AgentAction::Review {
            add_criterion_results(input, output)
        } else {
            output
        };
        Ok(ActionExecution {
            output,
            token_usage,
        })
    }
}

fn add_criterion_results(input: &str, output: String) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&output) else {
        return output;
    };
    if value.get("criterion_results").is_some() {
        return output;
    }
    let packet_json = input
        .split("## Authoritative Orc packet")
        .nth(1)
        .and_then(|text| text.find('{').map(|index| &text[index..]))
        .expect("review packet JSON");
    let packet: serde_json::Value = serde_json::from_str(packet_json).unwrap();
    let blockers = value
        .as_object_mut()
        .unwrap()
        .entry("blockers")
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .unwrap();
    for prior in packet["prior_blockers"].as_array().into_iter().flatten() {
        if prior["status"] != "resolved"
            && !blockers
                .iter()
                .any(|blocker| blocker["prior_blocker_id"] == prior["blocker_id"])
        {
            blockers.push(serde_json::json!({
                "id":prior["blocker_id"],"prior_blocker_id":prior["blocker_id"],
                "blocker_key":"explicitly-resolved-prior","requirement_ref":"prior blocker",
                "evidence":"Current bounded implementation evidence demonstrates resolution.",
                "severity":"unspecified","acceptance_condition":prior["acceptance_condition"],
                "status":"resolved","finding":"The prior concern is resolved in current evidence."
            }));
        }
    }
    let actionable = value["blockers"].as_array().is_some_and(|blockers| {
        blockers
            .iter()
            .any(|blocker| blocker["status"] != "resolved")
    }) || value["blocking_findings"]
        .as_array()
        .is_some_and(|findings| !findings.is_empty());
    value["criterion_results"] = serde_json::Value::Array(
        packet["task_contract"]["acceptance_criteria"]
            .as_array()
            .unwrap()
            .iter()
            .map(|criterion| serde_json::json!({
                "criterion_id": criterion["criterion_id"],
                "status": if actionable { "violated" } else { "satisfied" },
                "evidence": [{
                    "kind":"diff", "reference":"current_changes.diff",
                    "explanation":"The bounded current diff supplies concrete implementation evidence."
                }],
                "rationale": if actionable {
                    "The current implementation violates this required criterion."
                } else {
                    "The current implementation evidence satisfies this required criterion."
                }
            }))
            .collect(),
    );
    value.to_string()
}

fn pass_review() -> String {
    serde_json::json!({
        "verdict": "PASS", "findings": [], "blocking_findings": [],
        "non_blocking_findings": [], "severity": null, "revision_feedback": null,
        "blockers": []
    })
    .to_string()
}

fn revise_review(blockers: &[(&str, Option<&str>, &str)]) -> String {
    let blockers = blockers
        .iter()
        .map(|(key, prior, status)| {
            serde_json::json!({
                "id": key,
                "prior_blocker_id": prior,
                "blocker_key": key,
                "requirement_ref": format!("REQ-{key}"),
                "evidence": format!("semantic evidence for {key}"),
                "severity": "high",
                "acceptance_condition": format!("resolve {key}"),
                "status": status,
                "finding": format!("semantic blocker {key}")
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "verdict": "REVISE", "findings": [],
        "blocking_findings": ["semantic blockers remain"],
        "non_blocking_findings": [], "severity": "high",
        "revision_feedback": "resolve actionable blockers", "blockers": blockers
    })
    .to_string()
}

fn overrides() -> ActionOverrides {
    ActionOverrides {
        agent_id: Some("system-agent".into()),
        model: None,
        reasoning_effort: None,
    }
}

#[test]
fn automated_happy_path_is_one_shot_durable_and_accepted_explicitly() {
    let project = TestProject::new(&["acceptance-check"]);
    let worker = ScriptedWorker::new([WorkerTurn {
        file: "feature.txt",
        contents: "implemented\n",
        usage: Some(TokenUsage {
            total_tokens: 100,
            input_tokens: Some(80),
            output_tokens: Some(20),
            cached_input_tokens: Some(30),
        }),
    }]);
    let validation = SequenceValidationRunner::passing(1);

    let dispatch = dispatch_with_worker_on_db(
        &project.task,
        &worker,
        &project.db,
        project.path(),
        "system-agent",
        &validation,
    )
    .unwrap();
    assert_eq!(dispatch.task.status, TaskStatus::Review);
    assert_eq!(
        validation.commands.lock().unwrap().as_slice(),
        ["acceptance-check"]
    );
    assert!(
        project
            .db
            .list_agent_runs_for_task(&project.task)
            .unwrap()
            .iter()
            .all(|run| run.execution_class != "review")
    );
    assert_eq!(
        project
            .db
            .get_change_evidence(dispatch.run_id)
            .unwrap()
            .unwrap()
            .files[0]
            .path,
        "feature.txt"
    );
    assert_eq!(
        project
            .db
            .resolution_records(dispatch.run_id)
            .unwrap()
            .len(),
        1
    );

    let before_review = ProjectOperations::new(&project.db, project.path())
        .task_summary(&project.task)
        .unwrap()
        .unwrap();
    assert_eq!(before_review.validation.state, ValidationState::Passing);
    assert!(before_review.review.ready_for_review);
    assert_eq!(
        before_review.next_step,
        OperationalNextStep::RunSemanticReview
    );

    let backend = ScriptedBackend::new([(
        pass_review(),
        Some(TokenUsage {
            total_tokens: 50,
            input_tokens: Some(40),
            output_tokens: Some(10),
            cached_input_tokens: Some(15),
        }),
    )]);
    let (review_run, review) = project
        .app()
        .automated_review_with_backend(
            &project.task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    assert_eq!(review.verdict, "PASS");
    assert_eq!(
        project.db.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::AcceptanceReady
    );
    assert_eq!(project.db.resolution_records(review_run).unwrap().len(), 1);
    let prompt = &backend.calls.lock().unwrap()[0].1;
    assert!(prompt.contains("acceptance-check"));
    assert!(prompt.contains("feature.txt"));
    assert!(!prompt.to_ascii_lowercase().contains("run the tests"));
    assert!(!prompt.to_ascii_lowercase().contains("execute validation"));

    // PASS is not acceptance. Restart from disk and use the explicit command.
    let TestProject {
        directory,
        db,
        task,
    } = project;
    drop(db);
    let reopened = OrcApp::open(directory.path().join(".orc/orc.db"), directory.path()).unwrap();
    assert_eq!(
        reopened
            .operations()
            .task_summary(&task)
            .unwrap()
            .unwrap()
            .lifecycle,
        TaskStatus::AcceptanceReady
    );
    reopened.accept(&task).unwrap();
    let done = reopened.operations().task_summary(&task).unwrap().unwrap();
    assert_eq!(done.lifecycle, TaskStatus::Done);
    assert_eq!(done.next_step, OperationalNextStep::None);

    let economy = reopened.operations().economy_summary().unwrap();
    assert_eq!(economy.invocation_count, 2);
    assert_eq!(economy.token_usage.total_tokens, Some(150));
    assert_eq!(economy.token_usage.input_tokens, Some(120));
    assert_eq!(economy.token_usage.cached_input_tokens, Some(45));
    assert_eq!(economy.token_usage.uncached_input_tokens, Some(75));
    assert_eq!(economy.accepted_tasks, 1);

    // Historical resolution is immutable even if current configuration moves.
    Database::open(directory.path().join(".orc/orc.db"))
        .unwrap()
        .set_agent_model("system-agent", "model-new")
        .unwrap();
    let detail = reopened.operations().task_detail(&task).unwrap().unwrap();
    assert_eq!(detail.resolutions.len(), 2);
    assert!(
        detail
            .resolutions
            .iter()
            .all(|resolution| resolution.model.as_deref() == Some("model-economy"))
    );
}

#[test]
fn review_revision_converges_multiple_blockers_across_restart() {
    let project = TestProject::new(&["acceptance-check"]);
    dispatch_with_worker_on_db(
        &project.task,
        &ScriptedWorker::new([WorkerTurn {
            file: "feature.txt",
            contents: "initial\n",
            usage: None,
        }]),
        &project.db,
        project.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    let a = blocker_id("A");
    let b = blocker_id("B");
    let backend = ScriptedBackend::new([
        (
            revise_review(&[("A", None, "new"), ("B", None, "new")]),
            None,
        ),
        (
            revise_review(&[("A", Some(&a), "resolved"), ("B", Some(&b), "unresolved")]),
            None,
        ),
        (
            revise_review(&[("A", Some(&a), "regression"), ("B", Some(&b), "resolved")]),
            None,
        ),
        (pass_review(), None),
    ]);
    project
        .app()
        .automated_review_with_backend(
            &project.task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    assert_eq!(
        project.db.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::RevisionRequired
    );
    assert_eq!(
        project
            .db
            .review_blocker_ledger(&project.task)
            .unwrap()
            .len(),
        2
    );

    let first_revision = ScriptedWorker::new([WorkerTurn {
        file: "feature.txt",
        contents: "A fixed\n",
        usage: None,
    }]);
    revise_with_worker_on_db(
        &project.task,
        "fix A",
        &first_revision,
        &project.db,
        project.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    let first_packet = &first_revision.prompts.lock().unwrap()[0];
    assert!(first_packet.contains(&a));
    assert!(first_packet.contains(&b));
    project
        .app()
        .automated_review_with_backend(
            &project.task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap();

    let TestProject {
        directory,
        db,
        task,
    } = project;
    drop(db);
    let reopened_db = Database::open(directory.path().join(".orc/orc.db")).unwrap();
    let (_, contract, _) = reopened_db
        .actionable_revision_contract(&task)
        .unwrap()
        .unwrap();
    let contract: serde_json::Value = serde_json::from_str(&contract).unwrap();
    assert_eq!(contract["unresolved"][0]["blocker_id"], b);
    assert_eq!(contract["regression_constraints"][0]["blocker_id"], a);

    let second_revision = ScriptedWorker::new([WorkerTurn {
        file: "feature.txt",
        contents: "A and B fixed\n",
        usage: None,
    }]);
    revise_with_worker_on_db(
        &task,
        "fix B",
        &second_revision,
        &reopened_db,
        directory.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    let second_packet = &second_revision.prompts.lock().unwrap()[0];
    assert!(second_packet.contains(&b));
    assert!(!second_packet.contains("semantic blocker A"));
    OrcApp::open(directory.path().join(".orc/orc.db"), directory.path())
        .unwrap()
        .automated_review_with_backend(
            &task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    assert_eq!(
        reopened_db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::RevisionRequired
    );
    assert_eq!(
        reopened_db
            .review_blocker_ledger(&task)
            .unwrap()
            .iter()
            .find(|blocker| blocker.blocker_id == a)
            .unwrap()
            .status,
        "regression"
    );
    revise_with_worker_on_db(
        &task,
        "repair regressed A",
        &ScriptedWorker::new([WorkerTurn {
            file: "feature.txt",
            contents: "A and B fixed without regression\n",
            usage: None,
        }]),
        &reopened_db,
        directory.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    OrcApp::open(directory.path().join(".orc/orc.db"), directory.path())
        .unwrap()
        .automated_review_with_backend(
            &task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    assert_eq!(
        reopened_db.get_task(&task).unwrap().unwrap().status,
        TaskStatus::AcceptanceReady
    );
    assert!(
        reopened_db
            .review_blocker_ledger(&task)
            .unwrap()
            .iter()
            .all(|blocker| blocker.status == "resolved")
    );
    for run in reopened_db.list_agent_runs_for_task(&task).unwrap() {
        let invocations = reopened_db.provider_invocations(run.id).unwrap();
        assert_eq!(
            invocations.len(),
            1,
            "run {} has one provider invocation",
            run.id
        );
        assert_eq!(
            reopened_db.resolution_records(run.id).unwrap().len(),
            invocations.len(),
            "run {} has exactly one authoritative resolution per invocation",
            run.id
        );
    }
    OrcApp::open(directory.path().join(".orc/orc.db"), directory.path())
        .unwrap()
        .accept(&task)
        .unwrap();
}

#[test]
fn validation_repair_is_bounded_focused_and_never_auto_reviews() {
    let project = TestProject::new(&["check-one", "check-two"]);
    let worker = ScriptedWorker::new([
        WorkerTurn {
            file: "feature.txt",
            contents: "broken\n",
            usage: None,
        },
        WorkerTurn {
            file: "feature.txt",
            contents: "repaired\n",
            usage: None,
        },
    ]);
    let validation = SequenceValidationRunner::new([
        Ok(validation_step(false)),
        Ok(validation_step(true)),
        Ok(validation_step(true)),
        Ok(validation_step(true)),
    ]);
    let summary = dispatch_with_worker_on_db(
        &project.task,
        &worker,
        &project.db,
        project.path(),
        "system-agent",
        &validation,
    )
    .unwrap();
    assert_eq!(
        validation.commands.lock().unwrap().as_slice(),
        ["check-one", "check-two", "check-one", "check-two"]
    );
    let prompts = worker.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert!(prompts[1].contains("check-one"));
    assert!(prompts[1].contains("current deterministic failure"));
    assert!(!prompts[1].contains("check-two: FAILED"));
    assert_eq!(
        project.db.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::Review
    );
    assert_eq!(
        project.db.resolution_records(summary.run_id).unwrap().len(),
        2
    );
    assert!(
        project
            .db
            .list_agent_runs_for_task(&project.task)
            .unwrap()
            .iter()
            .all(|run| run.execution_class != "review")
    );
    let current = ProjectOperations::new(&project.db, project.path())
        .task_summary(&project.task)
        .unwrap()
        .unwrap();
    assert_eq!(current.validation.state, ValidationState::Passing);
}

#[test]
fn infrastructure_validation_failure_blocks_without_repair_review_or_escalation() {
    let project = TestProject::new(&["acceptance-check"]);
    let worker = ScriptedWorker::new([WorkerTurn {
        file: "feature.txt",
        contents: "implemented\n",
        usage: None,
    }]);
    let validation = SequenceValidationRunner::new([Err("validation host unavailable".into())]);
    let error = dispatch_with_worker_on_db(
        &project.task,
        &worker,
        &project.db,
        project.path(),
        "system-agent",
        &validation,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("infrastructure"));
    assert_eq!(worker.prompts.lock().unwrap().len(), 1);
    assert_eq!(
        project.db.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::Blocked
    );
    assert!(
        project
            .db
            .pending_escalation_request(&project.task)
            .unwrap()
            .is_none()
    );
    let summary = ProjectOperations::new(&project.db, project.path())
        .task_summary(&project.task)
        .unwrap()
        .unwrap();
    assert_eq!(
        summary.validation.state,
        ValidationState::InfrastructureFailure
    );
    assert_eq!(summary.next_step, OperationalNextStep::ResolveBlocker);
    assert!(
        project
            .db
            .list_agent_runs_for_task(&project.task)
            .unwrap()
            .iter()
            .all(|run| run.execution_class != "review")
    );
}

#[test]
fn stale_validation_and_stale_pass_are_rejected_before_provider_or_acceptance() {
    let project = TestProject::new(&["acceptance-check"]);
    dispatch_with_worker_on_db(
        &project.task,
        &ScriptedWorker::new([WorkerTurn {
            file: "feature.txt",
            contents: "implemented\n",
            usage: None,
        }]),
        &project.db,
        project.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    std::fs::write(
        project.worktree(&project.task).join("after-validation.txt"),
        "stale\n",
    )
    .unwrap();
    let backend = ScriptedBackend::new([(pass_review(), None)]);
    let error = project
        .app()
        .automated_review_with_backend(
            &project.task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("stale"));
    assert!(backend.calls.lock().unwrap().is_empty());
    assert_eq!(
        ProjectOperations::new(&project.db, project.path())
            .task_summary(&project.task)
            .unwrap()
            .unwrap()
            .validation
            .state,
        ValidationState::Stale
    );

    // A separate task reaches PASS, then its worktree changes before Accept.
    let pass_task = project.add_task("stale pass");
    dispatch_with_worker_on_db(
        &pass_task,
        &ScriptedWorker::new([WorkerTurn {
            file: "pass.txt",
            contents: "implemented\n",
            usage: None,
        }]),
        &project.db,
        project.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    project
        .app()
        .automated_review_with_backend(
            &pass_task,
            &overrides(),
            &ScriptedBackend::new([(pass_review(), None)]),
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    std::fs::write(
        project.worktree(&pass_task).join("after-pass.txt"),
        "stale\n",
    )
    .unwrap();
    let error = project.app().accept(&pass_task).unwrap_err();
    assert!(format!("{error:#}").contains("stale"));
    assert_ne!(
        project.db.get_task(&pass_task).unwrap().unwrap().status,
        TaskStatus::Done
    );
}

const MANUAL_COMPLETION: &str = r#"{"step_results":[{"step_id":"manual","operations_performed":["create"],"affected_files":["worker-claimed.txt"],"observed":[],"verification_passed":[]}],"summary":"manual implementation complete"}"#;

#[test]
fn manual_run_and_patch_share_validation_review_and_restart_semantics() {
    let project = TestProject::new(&["acceptance-check"]);
    let manual = agent("manual-worker", MANUAL, None, 100, vec![AgentAction::Code]);
    project.db.insert_agent(&manual).unwrap();
    dispatch_manual(&project.task, &manual, &project.db, project.path()).unwrap();
    let run = project.db.list_agent_runs_for_task(&project.task).unwrap()[0].id;
    std::fs::write(
        project.worktree(&project.task).join("actual-manual.txt"),
        "manual\n",
    )
    .unwrap();
    let validation = SequenceValidationRunner::passing(1);
    submit_run_with_runner(
        &project.db,
        run,
        MANUAL_COMPLETION,
        project.path(),
        &validation,
    )
    .unwrap();
    assert_eq!(
        validation.commands.lock().unwrap().as_slice(),
        ["acceptance-check"]
    );
    assert_eq!(
        project.db.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::Review
    );
    assert_eq!(
        project.db.get_change_evidence(run).unwrap().unwrap().files[0].path,
        "actual-manual.txt"
    );

    // Real reopen, not an in-memory clone.
    let reopened = Database::open(project.path().join(".orc/orc.db")).unwrap();
    let manual_summary = ProjectOperations::new(&reopened, project.path())
        .task_summary(&project.task)
        .unwrap()
        .unwrap();
    assert_eq!(manual_summary.validation.state, ValidationState::Passing);
    assert!(manual_summary.review.ready_for_review);
    OrcApp::open(project.path().join(".orc/orc.db"), project.path())
        .unwrap()
        .automated_review_with_backend(
            &project.task,
            &overrides(),
            &ScriptedBackend::new([(pass_review(), None)]),
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    project.app().accept(&project.task).unwrap();

    let patch_task = project.add_task("manual patch parity");
    dispatch_manual(&patch_task, &manual, &project.db, project.path()).unwrap();
    let patch_run = project.db.list_agent_runs_for_task(&patch_task).unwrap()[0].id;
    let patch = "diff --git a/patch.txt b/patch.txt\nnew file mode 100644\n--- /dev/null\n+++ b/patch.txt\n@@ -0,0 +1 @@\n+patched\n";
    let patch_validation = SequenceValidationRunner::passing(1);
    submit_patch_with_runner(
        &project.db,
        patch_run,
        patch,
        project.path(),
        &patch_validation,
    )
    .unwrap();
    let patch_summary = ProjectOperations::new(&project.db, project.path())
        .task_summary(&patch_task)
        .unwrap()
        .unwrap();
    assert_eq!(
        patch_summary.validation.state,
        manual_summary.validation.state
    );
    assert_eq!(
        patch_summary.review.ready_for_review,
        manual_summary.review.ready_for_review
    );
    project
        .app()
        .automated_review_with_backend(
            &patch_task,
            &overrides(),
            &ScriptedBackend::new([(pass_review(), None)]),
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    project.app().accept(&patch_task).unwrap();
    assert_eq!(
        project.db.get_task(&patch_task).unwrap().unwrap().status,
        TaskStatus::Done
    );
}

#[test]
fn manual_validation_failure_persists_without_fabricated_repair_after_restart() {
    let project = TestProject::new(&["acceptance-check"]);
    let manual = agent("manual-worker", MANUAL, None, 100, vec![AgentAction::Code]);
    project.db.insert_agent(&manual).unwrap();
    dispatch_manual(&project.task, &manual, &project.db, project.path()).unwrap();
    let run = project.db.list_agent_runs_for_task(&project.task).unwrap()[0].id;
    let completion_only = SequenceValidationRunner::new([]);
    assert!(
        submit_run_with_runner(
            &project.db,
            run,
            MANUAL_COMPLETION,
            project.path(),
            &completion_only,
        )
        .is_err()
    );
    assert!(completion_only.commands.lock().unwrap().is_empty());
    assert_eq!(
        project.db.get_agent_run(run).unwrap().unwrap().status,
        "waiting_external"
    );
    std::fs::write(
        project.worktree(&project.task).join("manual.txt"),
        "broken\n",
    )
    .unwrap();
    let failure = SequenceValidationRunner::new([Ok(validation_step(false))]);
    assert!(
        submit_run_with_runner(
            &project.db,
            run,
            MANUAL_COMPLETION,
            project.path(),
            &failure,
        )
        .is_err()
    );
    assert_eq!(failure.commands.lock().unwrap().len(), 1);
    let reopened = Database::open(project.path().join(".orc/orc.db")).unwrap();
    assert_eq!(
        reopened.get_agent_run(run).unwrap().unwrap().status,
        "failed"
    );
    assert_eq!(
        reopened.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::Blocked
    );
    let summary = ProjectOperations::new(&reopened, project.path())
        .task_summary(&project.task)
        .unwrap()
        .unwrap();
    assert_eq!(summary.validation.state, ValidationState::Failing);
    assert!(
        reopened
            .pending_escalation_request(&project.task)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reopened
            .list_agent_runs_for_task(&project.task)
            .unwrap()
            .len(),
        1
    );
}

struct FakeQuotaRefresher {
    observations: BTreeMap<String, Result<i64, String>>,
    calls: Mutex<Vec<String>>,
}

impl QuotaRefresher for FakeQuotaRefresher {
    fn supports(&self, _: &AgentDefinition) -> bool {
        true
    }

    fn refresh(&self, db: &Database, agent: &AgentDefinition) -> std::result::Result<(), String> {
        self.calls.lock().unwrap().push(agent.id.clone());
        match self.observations.get(&agent.id).cloned().unwrap_or(Ok(100)) {
            Ok(remaining) => db
                .set_agent_quota(&agent.id, remaining, None)
                .map_err(|error| error.to_string())
                .and_then(|updated| {
                    updated
                        .then_some(())
                        .ok_or_else(|| "agent disappeared during quota refresh".into())
                }),
            Err(error) => Err(error),
        }
    }
}

fn economy_project() -> TestProject {
    let project = TestProject::new(&[]);
    project
        .db
        .set_agent_availability(
            "eligibility-anchor",
            "unavailable",
            Some("economy fixture controls all candidates"),
        )
        .unwrap();
    project
        .db
        .set_economy_cost_configuration(&EconomyCostConfiguration {
            model_costs: BTreeMap::from([
                ("model-cheap".into(), 1.0),
                ("model-peer".into(), 1.0),
                ("model-expensive".into(), 5.0),
            ]),
            unknown_tier: EconomyTier::Unknown,
        })
        .unwrap();
    project.db.set_quota_reserve(10).unwrap();
    project
}

#[test]
fn economy_uses_cheapest_tier_then_deterministic_within_tier_and_obeys_overrides() {
    let project = economy_project();
    project
        .db
        .insert_agent(&agent(
            "cheap-low-priority",
            AUTOMATED,
            Some("model-cheap"),
            10,
            vec![AgentAction::Code],
        ))
        .unwrap();
    project
        .db
        .insert_agent(&agent(
            "cheap-high-priority",
            AUTOMATED,
            Some("model-peer"),
            90,
            vec![AgentAction::Code],
        ))
        .unwrap();
    project
        .db
        .insert_agent(&agent(
            "expensive-high-priority",
            AUTOMATED,
            Some("model-expensive"),
            1000,
            vec![AgentAction::Code],
        ))
        .unwrap();
    project
        .db
        .set_agent_availability(
            "system-agent",
            "unavailable",
            Some("fixture excludes baseline"),
        )
        .unwrap();
    let task = project.db.get_task(&project.task).unwrap().unwrap();
    let refresh = FakeQuotaRefresher {
        observations: BTreeMap::new(),
        calls: Mutex::new(Vec::new()),
    };
    let decision = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides::default(),
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:economy",
        &HashSet::new(),
        &refresh,
    )
    .unwrap();
    let selected = decision.resolution.unwrap();
    assert_eq!(selected.agent.id, "cheap-high-priority");
    assert_eq!(selected.record.tier, EconomyTier::Default);

    project
        .db
        .set_agent_availability(
            "cheap-high-priority",
            "unavailable",
            Some("capacity unavailable"),
        )
        .unwrap();
    project
        .db
        .set_agent_availability(
            "cheap-low-priority",
            "unavailable",
            Some("capacity unavailable"),
        )
        .unwrap();
    let promoted = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides::default(),
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:economy-promoted",
        &HashSet::new(),
        &refresh,
    )
    .unwrap()
    .resolution
    .unwrap();
    assert_eq!(promoted.agent.id, "expensive-high-priority");
    assert_eq!(promoted.record.tier, EconomyTier::Exceptional);

    project
        .db
        .set_agent_availability("cheap-low-priority", AVAILABLE, None)
        .unwrap();
    let explicit = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides {
            agent_id: Some("cheap-low-priority".into()),
            model: Some("model-peer".into()),
            effort: Some(ReasoningEffort::Medium),
        },
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:operator-override",
        &HashSet::new(),
        &refresh,
    )
    .unwrap()
    .resolution
    .unwrap();
    assert_eq!(explicit.agent.id, "cheap-low-priority");
    assert_eq!(explicit.execution.model.as_deref(), Some("model-peer"));
    assert_eq!(
        explicit.execution.reasoning_effort,
        Some(ReasoningEffort::Medium)
    );
    assert!(explicit.record.source.contains("operator"));
    assert!(explicit.record.escalation.is_none());
    let override_run = project
        .db
        .create_agent_run_with_execution(
            project.db.get_project_id().unwrap().unwrap(),
            &project.task,
            &explicit.agent.id,
            AUTOMATED,
            AgentRunExecution {
                class: "implementation",
                model: explicit.execution.model.as_deref(),
                effort: explicit.execution.reasoning_effort,
                source: "acceptance-operator-override",
            },
        )
        .unwrap();
    let override_invocation = project
        .db
        .start_provider_invocation_with_resolution(
            override_run,
            "implementation",
            1,
            &explicit.record,
        )
        .unwrap();
    project
        .db
        .finish_provider_invocation(override_invocation, "completed", None)
        .unwrap();
    project
        .db
        .update_agent_run_status(override_run, "completed", None)
        .unwrap();
    let persisted = ProjectOperations::new(&project.db, project.path())
        .execution_detail(override_run)
        .unwrap()
        .unwrap()
        .latest_resolution
        .unwrap();
    assert_eq!(persisted.model.as_deref(), Some("model-peer"));
    assert_eq!(persisted.effort, Some(ReasoningEffort::Medium));
    assert!(persisted.operator_override);
    assert!(persisted.escalation_reason.is_none());

    let invalid = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides {
            agent_id: Some("cheap-high-priority".into()),
            ..EconomyOverrides::default()
        },
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:ineligible-override",
        &HashSet::new(),
        &refresh,
    )
    .unwrap();
    assert!(invalid.resolution.is_none());
}

#[test]
fn quota_refresh_distinguishes_sufficient_insufficient_and_failed_observations() {
    let project = economy_project();
    project
        .db
        .set_agent_availability(
            "system-agent",
            "unavailable",
            Some("quota fixture uses explicit candidates"),
        )
        .unwrap();
    let insert_stale = |id: &str| {
        let mut stale = agent(
            id,
            AUTOMATED,
            Some("model-cheap"),
            100,
            vec![AgentAction::Code],
        );
        stale.quota_remaining_percent = Some(0);
        stale.quota_checked_at = Some("2000-01-01 00:00:00".into());
        stale.quota_source = Some("provider".into());
        project.db.insert_agent(&stale).unwrap();
    };
    insert_stale("quota-enough");
    insert_stale("quota-low");
    insert_stale("quota-failed");
    let task = project.db.get_task(&project.task).unwrap().unwrap();
    let enough = FakeQuotaRefresher {
        observations: BTreeMap::from([("quota-enough".into(), Ok(80))]),
        calls: Mutex::new(Vec::new()),
    };
    let selected = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides {
            agent_id: Some("quota-enough".into()),
            ..EconomyOverrides::default()
        },
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:quota-refresh",
        &HashSet::new(),
        &enough,
    )
    .unwrap()
    .resolution
    .unwrap();
    assert_eq!(selected.agent.id, "quota-enough");
    assert_eq!(enough.calls.lock().unwrap().as_slice(), ["quota-enough"]);

    let insufficient = FakeQuotaRefresher {
        observations: BTreeMap::from([("quota-low".into(), Ok(5))]),
        calls: Mutex::new(Vec::new()),
    };
    let rejected = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides {
            agent_id: Some("quota-low".into()),
            ..EconomyOverrides::default()
        },
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:quota-low",
        &HashSet::new(),
        &insufficient,
    )
    .unwrap();
    assert!(rejected.resolution.is_none());

    let failed = FakeQuotaRefresher {
        observations: BTreeMap::from([(
            "quota-failed".into(),
            Err("quota endpoint unavailable".into()),
        )]),
        calls: Mutex::new(Vec::new()),
    };
    let error = resolve_task_economy_for_execution_with_refresher(
        &project.db,
        &task,
        AgentAction::Code,
        EconomyOverrides {
            agent_id: Some("quota-failed".into()),
            ..EconomyOverrides::default()
        },
        Some(AUTOMATED),
        None,
        None,
        None,
        TransportEligibility::IgnoreUnsupportedBackend,
        None,
        "acceptance:quota-failed",
        &HashSet::new(),
        &failed,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("quota observation failed"));
}

#[test]
fn validation_non_convergence_does_not_automatically_escalate_after_repair_exhaustion() {
    let project = TestProject::new(&["acceptance-check"]);
    project
        .db
        .set_agent_model("system-agent", "model-cheap")
        .unwrap();
    project
        .db
        .set_agent_action_profile(
            "system-agent",
            AgentAction::Code,
            Some("model-cheap"),
            Some(ReasoningEffort::Low),
        )
        .unwrap();
    project
        .db
        .set_economy_cost_configuration(&EconomyCostConfiguration {
            model_costs: BTreeMap::from([
                ("model-cheap".into(), 1.0),
                ("model-escalated".into(), 2.0),
            ]),
            unknown_tier: EconomyTier::Unknown,
        })
        .unwrap();
    project
        .db
        .insert_agent(&agent(
            "escalation-agent",
            AUTOMATED,
            Some("model-escalated"),
            1,
            vec![AgentAction::Code],
        ))
        .unwrap();
    let worker = ScriptedWorker::new((0..4).map(|attempt| WorkerTurn {
        file: "feature.txt",
        contents: match attempt {
            0 => "broken 0\n",
            1 => "broken 1\n",
            2 => "broken 2\n",
            _ => "broken 3\n",
        },
        usage: None,
    }));
    let failures = SequenceValidationRunner::new((0..4).map(|_| Ok(validation_step(false))));
    let error = dispatch_with_worker_on_db(
        &project.task,
        &worker,
        &project.db,
        project.path(),
        "system-agent",
        &failures,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("did not converge after 3 repairs"));
    assert_eq!(worker.prompts.lock().unwrap().len(), 4);
    assert_eq!(
        project.db.get_task(&project.task).unwrap().unwrap().status,
        TaskStatus::Blocked
    );
    assert!(
        project
            .db
            .list_agent_runs_for_task(&project.task)
            .unwrap()
            .iter()
            .all(|run| run.execution_class != "review")
    );

    assert!(
        project
            .db
            .pending_escalation_request(&project.task)
            .unwrap()
            .is_none()
    );
    assert!(
        project
            .db
            .get_task_execution_condition(&project.task)
            .unwrap()
            .is_none()
    );
}

#[test]
fn persisted_task_contract_and_risk_guards_reach_packets_without_strength_promotion() {
    let project = TestProject::new(&["acceptance-check"]);
    project
        .db
        .set_economy_cost_configuration(&EconomyCostConfiguration {
            model_costs: BTreeMap::from([("model-economy".into(), 1.0)]),
            unknown_tier: EconomyTier::Unknown,
        })
        .unwrap();
    let task = project.db.get_task(&project.task).unwrap().unwrap();
    project
        .db
        .set_task_proposal_metadata(
            &project.task,
            &orc::protocol::TaskProposal {
                local_id: project.task.clone(),
                title: task.title,
                objective: task.objective,
                role: task.role,
                priority: task.priority,
                depends_on: vec![],
                capabilities: vec!["code".into(), "command_execution".into()],
                scope_mode: None,
                context_files: vec!["README.md".into()],
                expected_changes: vec!["planned.txt".into()],
                unchanged: vec!["README.md remains compatible".into()],
                acceptance_criteria: vec!["ACCEPTANCE_MARKER survives planning".into()],
                required_tests: vec!["REQUIRED_TEST_MARKER is preserved".into()],
                validation: vec!["acceptance-check".into()],
                execution_hints: orc::protocol::ExecutionHints {
                    class: None,
                    model: None,
                    effort: Some("low".into()),
                    effort_reason: Some("risk changes guards, not model strength".into()),
                },
                risk_factors: orc::protocol::TaskRiskFactor::ALL.to_vec(),
            },
        )
        .unwrap();
    let worker = ScriptedWorker::new([WorkerTurn {
        file: "planned.txt",
        contents: "planned implementation\n",
        usage: None,
    }]);
    let dispatch = dispatch_with_worker_on_db(
        &project.task,
        &worker,
        &project.db,
        project.path(),
        "system-agent",
        &SequenceValidationRunner::passing(1),
    )
    .unwrap();
    let packet = &worker.prompts.lock().unwrap()[0];
    assert!(packet.contains("ACCEPTANCE_MARKER"));
    assert!(packet.contains("REQUIRED_TEST_MARKER"));
    assert_eq!(
        project
            .db
            .get_task(&project.task)
            .unwrap()
            .unwrap()
            .risk_policy()
            .required_guards
            .len(),
        orc::protocol::TaskRiskFactor::ALL.len()
    );
    let resolution = &project.db.resolution_records(dispatch.run_id).unwrap()[0];
    assert_eq!(resolution.tier, EconomyTier::Default);
    assert_eq!(resolution.effort, Some(ReasoningEffort::Low));
    assert!(resolution.escalation.is_none());

    let backend = ScriptedBackend::new([(pass_review(), None)]);
    project
        .app()
        .automated_review_with_backend(
            &project.task,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap();
    let review_packet = &backend.calls.lock().unwrap()[0].1;
    assert!(review_packet.contains("ACCEPTANCE_MARKER"));
    assert!(review_packet.contains("REQUIRED_TEST_MARKER"));
    let reopened = Database::open(project.path().join(".orc/orc.db")).unwrap();
    let contract = reopened.get_task_contract(&project.task).unwrap().unwrap();
    assert_eq!(
        contract.acceptance_criteria,
        ["ACCEPTANCE_MARKER survives planning"]
    );
    assert_eq!(
        contract.required_tests,
        ["REQUIRED_TEST_MARKER is preserved"]
    );
}

#[test]
fn invalid_lifecycle_commands_fail_before_provider_side_effects() {
    let project = TestProject::new(&["acceptance-check"]);
    project
        .db
        .set_agent_availability(
            "eligibility-anchor",
            "unavailable",
            Some("prove transport injection does not create eligibility"),
        )
        .unwrap();
    let ineligible_worker = ScriptedWorker::new([WorkerTurn {
        file: "must-not-run.txt",
        contents: "bad\n",
        usage: None,
    }]);
    assert!(
        dispatch_with_worker_on_db(
            &project.task,
            &ineligible_worker,
            &project.db,
            project.path(),
            "system-agent",
            &SequenceValidationRunner::new([]),
        )
        .is_err()
    );
    assert!(ineligible_worker.prompts.lock().unwrap().is_empty());
    project
        .db
        .set_agent_availability("eligibility-anchor", AVAILABLE, None)
        .unwrap();
    project
        .db
        .update_task_status(&project.task, TaskStatus::Active)
        .unwrap();
    let worker = ScriptedWorker::new([WorkerTurn {
        file: "should-not-exist",
        contents: "bad\n",
        usage: None,
    }]);
    assert!(
        dispatch_with_worker_on_db(
            &project.task,
            &worker,
            &project.db,
            project.path(),
            "system-agent",
            &SequenceValidationRunner::new([]),
        )
        .is_err()
    );
    assert!(worker.prompts.lock().unwrap().is_empty());
    project
        .db
        .update_task_status(&project.task, TaskStatus::Ready)
        .unwrap();
    assert!(
        revise_with_worker_on_db(
            &project.task,
            "invalid",
            &worker,
            &project.db,
            project.path(),
            "system-agent",
            &SequenceValidationRunner::new([]),
        )
        .is_err()
    );
    assert!(project.app().accept(&project.task).is_err());

    project
        .db
        .update_task_status(&project.task, TaskStatus::Done)
        .unwrap();
    let backend = ScriptedBackend::new([(pass_review(), None)]);
    assert!(
        project
            .app()
            .automated_review_with_backend(
                &project.task,
                &overrides(),
                &backend,
                &SequenceValidationRunner::new([]),
            )
            .is_err()
    );
    assert!(backend.calls.lock().unwrap().is_empty());

    let missing_validation = project.add_task("missing validation prerequisite");
    let manual = agent(
        "manual-missing-validation",
        MANUAL,
        None,
        1,
        vec![AgentAction::Code],
    );
    project.db.insert_agent(&manual).unwrap();
    dispatch_manual(&missing_validation, &manual, &project.db, project.path()).unwrap();
    std::fs::write(
        project.worktree(&missing_validation).join("manual.txt"),
        "unvalidated\n",
    )
    .unwrap();
    project
        .db
        .update_task_status(&missing_validation, TaskStatus::Review)
        .unwrap();
    let backend = ScriptedBackend::new([(pass_review(), None)]);
    let error = project
        .app()
        .automated_review_with_backend(
            &missing_validation,
            &overrides(),
            &backend,
            &SequenceValidationRunner::new([]),
        )
        .unwrap_err();
    assert!(format!("{error:#}").contains("validation"));
    assert!(backend.calls.lock().unwrap().is_empty());
}
