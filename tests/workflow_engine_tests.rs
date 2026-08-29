use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::Result;
use orc::app::OrcApp;
use orc::lead::LeadDecisionKind;
use orc::registry::{AgentAction, AgentDefinition, ReasoningEffort};
use orc::storage::Database;
use orc::storage::db::AgentRunExecution;
use orc::task::{CreateTaskInput, TaskPriority, TaskStatus};
use orc::workflow::{
    AcceptancePolicy, AppWorkflowActions, LeadOutcome, PlanOutcome, PlanReviewOutcome,
    ProviderOutcome, ReviewOutcome, WorkflowActions, WorkflowEngine, WorkflowPolicy, WorkflowStage,
    WorkflowStatus,
};
use tempfile::TempDir;

#[derive(Clone, Copy)]
enum Intake {
    Direct,
    Plan,
    UserThenDirect,
}

struct FakeActions<'a> {
    db: &'a Database,
    project: i64,
    intake: Intake,
    lead_calls: Mutex<usize>,
    plan_reviews: Mutex<VecDeque<LeadDecisionKind>>,
    plan_review_resolutions: Mutex<Vec<Option<String>>>,
    task_reviews: Mutex<VecDeque<&'static str>>,
    dispatches: Mutex<Vec<String>>,
    revisions: Mutex<usize>,
    create_dependencies: bool,
    dispatch_error: Option<&'static str>,
    leave_dispatch_active: bool,
    scheduling_block: Option<&'static str>,
}

impl<'a> FakeActions<'a> {
    fn new(db: &'a Database, project: i64, intake: Intake) -> Self {
        Self {
            db,
            project,
            intake,
            lead_calls: Mutex::new(0),
            plan_reviews: Mutex::new(VecDeque::from([LeadDecisionKind::Approve])),
            plan_review_resolutions: Mutex::new(Vec::new()),
            task_reviews: Mutex::new(VecDeque::from(["PASS"])),
            dispatches: Mutex::new(Vec::new()),
            revisions: Mutex::new(0),
            create_dependencies: false,
            dispatch_error: None,
            leave_dispatch_active: false,
            scheduling_block: None,
        }
    }

    fn semantic_run(&self, task_id: Option<&str>, action: &str, purpose: &str) -> Result<i64> {
        let run = self.db.create_project_action_run(
            self.project,
            task_id,
            action,
            "fake",
            AgentRunExecution {
                class: action,
                model: Some("fake-model"),
                effort: Some(ReasoningEffort::Low),
                source: "workflow-test",
            },
        )?;
        let invocation =
            self.db
                .start_provider_invocation(run, purpose, 1, Some(ReasoningEffort::Low))?;
        self.db
            .finish_provider_invocation(invocation, "completed", None)?;
        self.db.update_agent_run_status(run, "completed", None)?;
        Ok(run)
    }

    fn create_tasks(&self) -> Result<()> {
        if !self.db.list_tasks_for_project(self.project)?.is_empty() {
            return Ok(());
        }
        let first = self.db.create_task(
            self.project,
            &CreateTaskInput {
                title: "first".into(),
                objective: "implement first".into(),
                role: "general".into(),
                priority: TaskPriority::Normal,
                required_capabilities: vec![],
                scope_mode: None,
                context_files: vec![],
                expected_changes: vec![],
                dependencies: vec![],
            },
        )?;
        if self.create_dependencies {
            self.db.create_task(
                self.project,
                &CreateTaskInput {
                    title: "second".into(),
                    objective: "implement second".into(),
                    role: "general".into(),
                    priority: TaskPriority::Normal,
                    required_capabilities: vec![],
                    scope_mode: None,
                    context_files: vec![],
                    expected_changes: vec![],
                    dependencies: vec![first.clone()],
                },
            )?;
        }
        match self.scheduling_block {
            Some("unavailable") => {
                self.db.set_agent_availability(
                    "fake",
                    "unavailable",
                    Some("operator unavailable"),
                )?;
            }
            Some("quota") => {
                self.db.set_agent_quota("fake", 0, None)?;
            }
            Some("busy") => {
                self.db.create_agent_run(self.project, &first, "fake")?;
            }
            Some(other) => anyhow::bail!("unknown scheduling test state {other}"),
            None => {}
        }
        Ok(())
    }
}

impl WorkflowActions for FakeActions<'_> {
    fn discover(&self) -> Result<String> {
        Ok("snapshot-1".into())
    }

    fn lead(&self, _: &orc::workflow::WorkflowRun) -> Result<LeadOutcome> {
        let mut calls = self.lead_calls.lock().unwrap();
        *calls += 1;
        let kind = match (self.intake, *calls) {
            (Intake::Direct, _) => LeadDecisionKind::DirectTasks,
            (Intake::Plan, _) => LeadDecisionKind::PlanRequired,
            (Intake::UserThenDirect, 1) => LeadDecisionKind::UserDecisionRequired,
            (Intake::UserThenDirect, _) => LeadDecisionKind::DirectTasks,
        };
        Ok(LeadOutcome {
            decision_id: i64::try_from(*calls).unwrap(),
            provider_run_id: self.semantic_run(None, "lead", "lead")?,
            kind,
        })
    }

    fn apply_direct(&self) -> Result<()> {
        self.create_tasks()
    }

    fn plan(&self) -> Result<PlanOutcome> {
        Ok(PlanOutcome {
            plan_id: 10,
            provider_run_id: self.semantic_run(None, "plan", "plan")?,
        })
    }

    fn revise_plan(&self) -> Result<PlanOutcome> {
        Ok(PlanOutcome {
            plan_id: 11,
            provider_run_id: self.semantic_run(None, "plan", "plan")?,
        })
    }

    fn review_plan(
        &self,
        workflow: &orc::workflow::WorkflowRun,
        _: i64,
    ) -> Result<PlanReviewOutcome> {
        self.plan_review_resolutions
            .lock()
            .unwrap()
            .push(workflow.user_resolution.clone());
        Ok(PlanReviewOutcome {
            provider_run_id: self.semantic_run(None, "lead", "lead")?,
            decision: self
                .plan_reviews
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(LeadDecisionKind::Approve),
        })
    }

    fn apply_plan(&self) -> Result<()> {
        self.create_tasks()
    }

    fn dispatch(&self, task_id: &str) -> Result<ProviderOutcome> {
        self.dispatches.lock().unwrap().push(task_id.into());
        if let Some(error) = self.dispatch_error {
            anyhow::bail!(error)
        }
        self.db.update_task_status(task_id, TaskStatus::Active)?;
        let run = self.semantic_run(Some(task_id), "general", "implementation")?;
        if !self.leave_dispatch_active {
            self.db.update_task_status(task_id, TaskStatus::Review)?;
        }
        Ok(ProviderOutcome {
            provider_run_id: run,
        })
    }

    fn review(&self, task_id: &str) -> Result<ReviewOutcome> {
        let verdict = self
            .task_reviews
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or("PASS");
        Ok(ReviewOutcome {
            provider_run_id: self.semantic_run(Some(task_id), "review", "review")?,
            verdict: verdict.into(),
            feedback: (verdict == "REVISE").then(|| "fix exact blocker".into()),
        })
    }

    fn revise_task(&self, task_id: &str, _: &str) -> Result<ProviderOutcome> {
        *self.revisions.lock().unwrap() += 1;
        let run = self.semantic_run(Some(task_id), "general", "revision")?;
        self.db.update_task_status(task_id, TaskStatus::Review)?;
        Ok(ProviderOutcome {
            provider_run_id: run,
        })
    }

    fn accept(&self, task_id: &str) -> Result<()> {
        self.db.update_task_status(task_id, TaskStatus::Done)?;
        Ok(())
    }

    fn recover_dispatch(
        &self,
        workflow: &orc::workflow::WorkflowRun,
    ) -> Result<Option<ProviderOutcome>> {
        Ok(self
            .db
            .completed_workflow_provider_run(
                workflow.id,
                workflow.stage.as_str(),
                workflow.version,
                "implementation",
            )?
            .map(|provider_run_id| ProviderOutcome { provider_run_id }))
    }
}

fn setup() -> (TempDir, Database, i64) {
    let directory = tempfile::tempdir().unwrap();
    let db = Database::init(directory.path().join("orc.db")).unwrap();
    let project = db.create_project("workflow").unwrap();
    db.insert_agent(&AgentDefinition {
        id: "fake".into(),
        backend: "codex".into(),
        execution_mode: "automated".into(),
        display_name: "Fake".into(),
        enabled: true,
        priority: 1,
        capabilities: vec![],
        status: "available".into(),
        unavailable_reason: None,
        profile_path: Some("fake".into()),
        model: Some("fake-model".into()),
        reasoning_effort: Some(ReasoningEffort::Low),
        config_metadata: None,
        actions: vec![
            AgentAction::Code,
            AgentAction::Review,
            AgentAction::Plan,
            AgentAction::Lead,
        ],
        quota_remaining_percent: None,
        quota_reset_at: None,
        quota_checked_at: None,
        quota_source: None,
        quota_limits: None,
    })
    .unwrap();
    (directory, db, project)
}

fn automatic_policy() -> WorkflowPolicy {
    WorkflowPolicy {
        acceptance: AcceptancePolicy::Automatic,
        ..WorkflowPolicy::default()
    }
}

#[test]
fn direct_lifecycle_continues_through_dispatch_review_acceptance_and_done() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Direct);
    let run = WorkflowEngine::new(&db, &actions)
        .start(project, "ship it", automatic_policy())
        .unwrap();

    assert_eq!(run.status, WorkflowStatus::Completed);
    assert_eq!(
        db.list_tasks_for_project(project).unwrap()[0].status,
        TaskStatus::Done
    );
    assert_eq!(*actions.lead_calls.lock().unwrap(), 1);
    assert_eq!(actions.dispatches.lock().unwrap().len(), 1);
    let transitions = db.workflow_transitions(run.id).unwrap();
    assert!(
        transitions
            .iter()
            .any(|edge| edge.edge == "task_dispatched_and_validated")
    );
    assert!(transitions.iter().any(|edge| edge.edge == "task_reviewed"));
    assert!(
        transitions
            .iter()
            .filter(|edge| edge.deterministic)
            .all(|edge| edge.provider_run_id.is_none())
    );
    for provider_run in transitions.iter().filter_map(|edge| edge.provider_run_id) {
        let invocations = db.provider_invocations(provider_run).unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].workflow_id, Some(run.id));
        assert_eq!(invocations[0].selected_agent.as_deref(), Some("fake"));
        assert_eq!(invocations[0].selected_model.as_deref(), Some("fake-model"));
        assert_eq!(
            invocations[0].escalation_reason.as_deref(),
            Some("initial semantic invocation")
        );
    }
}

#[test]
fn plan_revision_and_task_revision_are_bounded_and_use_one_call_each() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Plan);
    *actions.plan_reviews.lock().unwrap() =
        VecDeque::from([LeadDecisionKind::RevisePlan, LeadDecisionKind::Approve]);
    *actions.task_reviews.lock().unwrap() = VecDeque::from(["REVISE", "PASS"]);
    let run = WorkflowEngine::new(&db, &actions)
        .start(project, "plan and ship", automatic_policy())
        .unwrap();

    assert_eq!(run.status, WorkflowStatus::Completed);
    assert_eq!(run.plan_revision_count, 1);
    assert_eq!(
        run.task_revision_count, 0,
        "acceptance resets per-task convergence state"
    );
    assert_eq!(*actions.revisions.lock().unwrap(), 1);
    let revision_runs = db
        .list_agent_runs(project, usize::MAX)
        .unwrap()
        .into_iter()
        .filter(|item| item.execution_class == "general" && item.task_id.is_some())
        .collect::<Vec<_>>();
    assert!(
        revision_runs
            .iter()
            .all(|item| db.provider_invocations(item.id).unwrap().len() == 1)
    );
}

#[test]
fn restart_between_every_committed_stage_resumes_without_repeating_provider_calls() {
    let (directory, db, project) = setup();
    let run = db
        .start_workflow(project, "restartable", &automatic_policy())
        .unwrap();
    let id = run.id;
    drop(db);

    loop {
        let reopened = Database::open(directory.path().join("orc.db")).unwrap();
        let actions = FakeActions::new(&reopened, project, Intake::Direct);
        let current = WorkflowEngine::new(&reopened, &actions)
            .continue_one(id)
            .unwrap();
        if current.status.is_terminal() {
            assert_eq!(current.status, WorkflowStatus::Completed);
            let provider_edges = reopened
                .workflow_transitions(id)
                .unwrap()
                .into_iter()
                .filter(|edge| !edge.deterministic)
                .collect::<Vec<_>>();
            assert_eq!(provider_edges.len(), 3);
            break;
        }
    }
}

#[test]
fn completed_provider_call_is_reconciled_after_crash_before_stage_commit() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Direct);
    let run = db
        .start_workflow(project, "crash window", &automatic_policy())
        .unwrap();
    let engine = WorkflowEngine::new(&db, &actions);
    let mut current = run;
    while current.stage != WorkflowStage::Dispatch {
        current = engine.continue_one(current.id).unwrap();
    }
    let task_id = current.current_task_id.clone().unwrap();
    actions.dispatch(&task_id).unwrap();
    assert_eq!(actions.dispatches.lock().unwrap().len(), 1);
    assert!(
        db.completed_workflow_provider_run(
            current.id,
            current.stage.as_str(),
            current.version,
            "implementation",
        )
        .unwrap()
        .is_some()
    );

    // Simulate process loss before the Dispatch -> Review workflow edge was
    // committed. Continuation consumes the persisted invocation and task
    // state instead of invoking Worker a second time.
    let completed = engine.continue_run(current.id).unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(actions.dispatches.lock().unwrap().len(), 1);
}

#[test]
fn provider_completion_without_a_completed_semantic_run_stops_recovery() {
    let (directory, db, project) = setup();
    let task = db
        .create_task(
            project,
            &CreateTaskInput {
                title: "interrupted".into(),
                objective: "do not replay an interrupted provider call".into(),
                role: "general".into(),
                priority: TaskPriority::Normal,
                required_capabilities: vec![],
                scope_mode: None,
                context_files: vec![],
                expected_changes: vec![],
                dependencies: vec![],
            },
        )
        .unwrap();
    db.update_task_status(&task, TaskStatus::Active).unwrap();
    let workflow = db
        .start_workflow(project, "interrupted provider", &automatic_policy())
        .unwrap();
    let mut dispatching = workflow.clone();
    dispatching.stage = WorkflowStage::Dispatch;
    dispatching.current_task_id = Some(task.clone());
    let dispatching = db
        .commit_workflow_transition(
            &workflow,
            &dispatching,
            "test_dispatch_selected",
            true,
            None,
            None,
        )
        .unwrap();
    let provider_run = db
        .create_agent_run_with_execution(
            project,
            &task,
            "fake",
            "automated",
            AgentRunExecution {
                class: "general",
                model: Some("fake-model"),
                effort: Some(ReasoningEffort::Low),
                source: "workflow-test",
            },
        )
        .unwrap();
    let invocation = db
        .start_provider_invocation(
            provider_run,
            "implementation",
            1,
            Some(ReasoningEffort::Low),
        )
        .unwrap();
    db.finish_provider_invocation(invocation, "completed", None)
        .unwrap();
    drop(db);

    let app = OrcApp::open(directory.path().join("orc.db"), directory.path()).unwrap();
    let actions = AppWorkflowActions::new(&app);
    let error = actions.recover_dispatch(&dispatching).unwrap_err();
    assert!(error.to_string().contains("ended 'running'"));
    assert_eq!(
        app.workflow_run(dispatching.id).unwrap().unwrap().stage,
        WorkflowStage::Dispatch
    );
}

#[test]
fn user_gate_stops_and_resumes_without_hidden_transition() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::UserThenDirect);
    let engine = WorkflowEngine::new(&db, &actions);
    let waiting = engine.start(project, "ask me", automatic_policy()).unwrap();
    assert_eq!(waiting.status, WorkflowStatus::WaitingUser);
    assert_eq!(waiting.stage, WorkflowStage::Lead);

    let completed = engine.resolve_user_gate(waiting.id, "continue").unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(*actions.lead_calls.lock().unwrap(), 2);
}

#[test]
fn plan_review_user_response_is_authoritative_context_for_the_resumed_review() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Plan);
    *actions.plan_reviews.lock().unwrap() = VecDeque::from([
        LeadDecisionKind::UserDecisionRequired,
        LeadDecisionKind::Approve,
    ]);
    let engine = WorkflowEngine::new(&db, &actions);
    let waiting = engine
        .start(project, "ask during plan review", automatic_policy())
        .unwrap();
    assert_eq!(waiting.status, WorkflowStatus::WaitingUser);
    assert_eq!(waiting.stage, WorkflowStage::PlanReview);

    let completed = engine
        .resolve_user_gate(waiting.id, "use postgres")
        .unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(
        &*actions.plan_review_resolutions.lock().unwrap(),
        &[None, Some("use postgres".into())]
    );
    assert!(completed.user_resolution.is_none());
}

#[test]
fn approval_gate_rejects_ambiguous_resolution_without_advancing() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Direct);
    let policy = WorkflowPolicy::default();
    let engine = WorkflowEngine::new(&db, &actions);
    let waiting = engine.start(project, "approval", policy).unwrap();
    assert_eq!(waiting.status, WorkflowStatus::AcceptanceReady);
    let version = waiting.version;

    let error = engine
        .resolve_user_gate(waiting.id, "maybe later")
        .unwrap_err();
    assert!(error.to_string().contains("explicit accept"));
    let unchanged = db.get_workflow(waiting.id).unwrap().unwrap();
    assert_eq!(unchanged.status, WorkflowStatus::AcceptanceReady);
    assert_eq!(unchanged.version, version);
}

#[test]
fn dependency_dispatch_is_ordered_and_unavailable_or_budget_failures_do_not_retry() {
    let (_directory, db, project) = setup();
    let mut actions = FakeActions::new(&db, project, Intake::Direct);
    actions.create_dependencies = true;
    let completed = WorkflowEngine::new(&db, &actions)
        .start(project, "dependencies", automatic_policy())
        .unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(&*actions.dispatches.lock().unwrap(), &["T-0001", "T-0002"]);

    let (_other_directory, other_db, other_project) = setup();
    let mut exhausted = FakeActions::new(&other_db, other_project, Intake::Direct);
    exhausted.dispatch_error = Some("provider token budget exhausted before implementation");
    let stopped = WorkflowEngine::new(&other_db, &exhausted)
        .start(other_project, "budget", automatic_policy())
        .unwrap();
    assert_eq!(stopped.status, WorkflowStatus::BudgetExhausted);
    assert_eq!(exhausted.dispatches.lock().unwrap().len(), 1);
}

#[test]
fn external_dispatch_resume_and_scheduler_stops_are_deterministic() {
    let (_directory, db, project) = setup();
    let mut manual = FakeActions::new(&db, project, Intake::Direct);
    manual.leave_dispatch_active = true;
    let waiting = WorkflowEngine::new(&db, &manual)
        .start(project, "manual completion", automatic_policy())
        .unwrap();
    assert_eq!(waiting.status, WorkflowStatus::WaitingExternal);
    let task = waiting.current_task_id.as_deref().unwrap();
    db.update_task_status(task, TaskStatus::Review).unwrap();
    let completed = WorkflowEngine::new(&db, &manual)
        .continue_run(waiting.id)
        .unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(manual.dispatches.lock().unwrap().len(), 1);
    assert!(
        db.workflow_transitions(waiting.id)
            .unwrap()
            .iter()
            .any(|edge| edge.edge == "external_task_ready_for_review"
                && edge.deterministic
                && edge.provider_run_id.is_none())
    );

    for state in ["unavailable", "quota", "busy"] {
        let (_directory, db, project) = setup();
        let mut actions = FakeActions::new(&db, project, Intake::Direct);
        actions.scheduling_block = Some(state);
        let stopped = WorkflowEngine::new(&db, &actions)
            .start(project, state, automatic_policy())
            .unwrap();
        assert_eq!(stopped.status, WorkflowStatus::Blocked, "state={state}");
        assert!(actions.dispatches.lock().unwrap().is_empty());
    }
}

#[test]
fn non_convergence_cancellation_and_supersession_are_explicit() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Direct);
    *actions.task_reviews.lock().unwrap() = VecDeque::from(["REVISE", "REVISE"]);
    let policy = WorkflowPolicy {
        acceptance: AcceptancePolicy::Automatic,
        max_task_revisions: 1,
        ..WorkflowPolicy::default()
    };
    let stopped = WorkflowEngine::new(&db, &actions)
        .start(project, "does not converge", policy)
        .unwrap();
    assert_eq!(stopped.status, WorkflowStatus::NonConvergent);
    assert_eq!(*actions.revisions.lock().unwrap(), 1);

    let first = db
        .start_workflow(project, "old", &WorkflowPolicy::default())
        .unwrap();
    let second = db
        .start_workflow(project, "new", &WorkflowPolicy::default())
        .unwrap();
    assert_eq!(
        db.get_workflow(first.id).unwrap().unwrap().status,
        WorkflowStatus::Superseded
    );
    let cancelled = WorkflowEngine::new(&db, &actions)
        .cancel(second.id, Some("operator stop"))
        .unwrap();
    assert_eq!(cancelled.status, WorkflowStatus::Cancelled);
}
