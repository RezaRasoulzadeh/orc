use std::collections::VecDeque;
use std::fs;
use std::sync::Mutex;

use anyhow::Result;
use orc::app::OrcApp;
use orc::lead::LeadDecisionKind;
use orc::registry::{AgentAction, AgentDefinition, EconomyTier, ReasoningEffort, ResolutionRecord};
use orc::storage::Database;
use orc::storage::db::AgentRunExecution;
use orc::task::{CreateTaskInput, TaskPriority, TaskStatus};
use orc::workflow::{
    AcceptancePolicy, AppWorkflowActions, ControllerPlanOutcome, ControllerPlanReviewDecision,
    ControllerPlanReviewOutcome, LeadOutcome, PlanOutcome, PlanReviewOutcome, ProviderOutcome,
    ReviewOutcome, WorkflowActions, WorkflowEngine, WorkflowPolicy, WorkflowStage, WorkflowStatus,
};
use tempfile::TempDir;

#[derive(Clone, Copy)]
enum Intake {
    Direct,
    Plan,
    UserThenDirect,
}

fn fake_resolution(purpose: &str) -> ResolutionRecord {
    ResolutionRecord {
        selected_agent: "fake".into(),
        selected_model: Some("fake-model".into()),
        effort: Some(ReasoningEffort::Low),
        tier: EconomyTier::Unknown,
        source: "workflow-test".into(),
        escalation_reason: None,
        input_lineage: format!("workflow-test:{purpose}"),
        escalation: None,
    }
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
    controller_plan_calls: Mutex<usize>,
    controller_revision_calls: Mutex<usize>,
    controller_review_calls: Mutex<usize>,
    controller_plan_persisted: Mutex<bool>,
    controller_revision_persisted: Mutex<bool>,
    controller_review_persisted: Mutex<bool>,
    controller_review_plan_id: Mutex<Option<i64>>,
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
            controller_plan_calls: Mutex::new(0),
            controller_revision_calls: Mutex::new(0),
            controller_review_calls: Mutex::new(0),
            controller_plan_persisted: Mutex::new(false),
            controller_revision_persisted: Mutex::new(false),
            controller_review_persisted: Mutex::new(false),
            controller_review_plan_id: Mutex::new(None),
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
        let invocation = self.db.start_provider_invocation_with_resolution(
            run,
            purpose,
            1,
            &fake_resolution(purpose),
        )?;
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

    fn controller_plan(
        &self,
        _: &orc::workflow::WorkflowRun,
        _: &mut dyn orc::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanOutcome>> {
        *self.controller_plan_calls.lock().unwrap() += 1;
        *self.controller_plan_persisted.lock().unwrap() = true;
        Ok(Some(ControllerPlanOutcome { plan_id: 10 }))
    }

    fn controller_plan_revision(
        &self,
        _: &orc::workflow::WorkflowRun,
        _: &mut dyn orc::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanOutcome>> {
        *self.controller_revision_calls.lock().unwrap() += 1;
        *self.controller_revision_persisted.lock().unwrap() = true;
        Ok(Some(ControllerPlanOutcome { plan_id: 11 }))
    }

    fn controller_plan_review(
        &self,
        _: &orc::workflow::WorkflowRun,
        plan_id: i64,
        _: &mut dyn orc::local_runtime::LocalInferenceRuntime,
    ) -> Result<Option<ControllerPlanReviewOutcome>> {
        *self.controller_review_calls.lock().unwrap() += 1;
        *self.controller_review_persisted.lock().unwrap() = true;
        *self.controller_review_plan_id.lock().unwrap() = Some(plan_id);
        let decision = self
            .plan_reviews
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(LeadDecisionKind::Approve);
        Ok(Some(ControllerPlanReviewOutcome {
            decision: match decision {
                LeadDecisionKind::Approve => ControllerPlanReviewDecision::Approve,
                LeadDecisionKind::RevisePlan => ControllerPlanReviewDecision::RevisePlan,
                LeadDecisionKind::UserDecisionRequired => {
                    ControllerPlanReviewDecision::UserDecisionRequired
                }
                other => anyhow::bail!("unsupported controller test decision {other:?}"),
            },
        }))
    }

    fn recover_controller_plan(
        &self,
        workflow: &orc::workflow::WorkflowRun,
    ) -> Result<Option<ControllerPlanOutcome>> {
        let persisted = match workflow.stage {
            WorkflowStage::Planner => *self.controller_plan_persisted.lock().unwrap(),
            WorkflowStage::PlannerRevision => *self.controller_revision_persisted.lock().unwrap(),
            _ => false,
        };
        Ok(persisted.then_some(ControllerPlanOutcome {
            plan_id: if workflow.stage == WorkflowStage::Planner {
                10
            } else {
                11
            },
        }))
    }

    fn recover_controller_plan_review(
        &self,
        workflow: &orc::workflow::WorkflowRun,
    ) -> Result<Option<ControllerPlanReviewOutcome>> {
        if workflow.user_resolution.is_some()
            || !*self.controller_review_persisted.lock().unwrap()
            || *self.controller_review_plan_id.lock().unwrap() != workflow.plan_id
        {
            return Ok(None);
        }
        Ok(Some(ControllerPlanReviewOutcome {
            decision: ControllerPlanReviewDecision::Approve,
        }))
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

struct NoopControllerRuntime;

impl orc::local_runtime::LocalInferenceRuntime for NoopControllerRuntime {
    fn infer(
        &mut self,
        _: &orc::local_runtime::LocalInferenceRequest,
    ) -> Result<orc::local_runtime::LocalInferenceResponse, orc::local_runtime::LocalInferenceError>
    {
        Err(orc::local_runtime::LocalInferenceError::Backend(
            "test action consumes the Controller boundary without direct runtime inference".into(),
        ))
    }
}

struct AppControllerRuntime {
    responses: VecDeque<orc::local_runtime::LocalInferenceResponse>,
    calls: usize,
}

impl orc::local_runtime::LocalInferenceRuntime for AppControllerRuntime {
    fn infer(
        &mut self,
        _: &orc::local_runtime::LocalInferenceRequest,
    ) -> Result<orc::local_runtime::LocalInferenceResponse, orc::local_runtime::LocalInferenceError>
    {
        self.calls += 1;
        self.responses.pop_front().ok_or_else(|| {
            orc::local_runtime::LocalInferenceError::Backend(
                "no Controller fixture response".into(),
            )
        })
    }
}

fn controller_plan_response(objective: &str) -> orc::local_runtime::LocalInferenceResponse {
    orc::local_runtime::LocalInferenceResponse::structured(
        "controller fixture",
        serde_json::json!({
            "plan": {
                "protocol_version": orc::protocol::PROTOCOL_VERSION,
                "objective": objective,
                "assumptions": [],
                "risks": [],
                "questions": [],
                "tasks": []
            },
            "rationale": "bounded fixture plan",
            "uncertainty": null
        }),
    )
}

fn controller_plan_result(objective: &str) -> orc::controller_planning::ControllerPlanResult {
    orc::controller_planning::ControllerPlanResult {
        plan: orc::protocol::PlanResponse {
            protocol_version: orc::protocol::PROTOCOL_VERSION,
            objective: objective.into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        },
        rationale: "workflow-bound fixture plan".into(),
        uncertainty: None,
    }
}

fn persist_workflow_plan(app: &OrcApp, workflow_id: i64, objective: &str) -> i64 {
    let proposal = app
        .propose_controller_plan_persistence_for_workflow(
            workflow_id,
            &controller_plan_result(objective),
        )
        .unwrap();
    let authorization = app.authorize_controller_plan_persistence(&proposal);
    match app.execute_authorized_controller_plan_persistence(&proposal, Some(authorization)) {
        orc::controller_plan_persistence::ControllerPlanPersistenceResult::Persisted {
            plan_id,
            ..
        } => plan_id,
        other => panic!("unexpected Controller Plan persistence result: {other:?}"),
    }
}

fn persist_workflow_review(
    app: &OrcApp,
    workflow_id: i64,
    plan_id: i64,
    resolution: Option<&str>,
    decision: orc::controller_plan_review::ControllerPlanReviewDecision,
) -> i64 {
    let result = orc::controller_plan_review::ControllerPlanReviewResult {
        decision,
        details: "workflow-bound fixture review".into(),
        revision_feedback: (decision
            == orc::controller_plan_review::ControllerPlanReviewDecision::RevisePlan)
            .then_some("workflow-bound fixture feedback".into()),
    };
    let proposal = app
        .propose_controller_plan_review_persistence_for_workflow(
            workflow_id,
            plan_id,
            resolution,
            &result,
        )
        .unwrap();
    let authorization = app.authorize_controller_plan_review_persistence(&proposal);
    match app.execute_authorized_controller_plan_review_persistence(&proposal, Some(authorization))
    {
        orc::controller_plan_review_persistence::ControllerPlanReviewPersistenceResult::Persisted {
            review_id,
            ..
        } => review_id,
        other => panic!("unexpected Controller review persistence result: {other:?}"),
    }
}

fn persist_workflow_revision(
    app: &OrcApp,
    workflow_id: i64,
    parent: &orc::storage::db::PersistedPlan,
    review_id: i64,
    objective: &str,
) -> i64 {
    let result = orc::controller_plan_revision::ControllerPlanRevisionResult {
        parent_plan_id: parent.id,
        parent_plan_version: parent.version,
        review_id,
        plan: orc::protocol::PlanResponse {
            protocol_version: orc::protocol::PROTOCOL_VERSION,
            objective: objective.into(),
            assumptions: vec![],
            risks: vec![],
            questions: vec![],
            tasks: vec![],
        },
    };
    let proposal = app
        .propose_controller_plan_revision_persistence_for_workflow(workflow_id, &result)
        .unwrap();
    let authorization = app.authorize_controller_plan_revision_persistence(&proposal);
    match app.execute_authorized_controller_plan_revision_persistence(&proposal, Some(authorization))
    {
        orc::controller_plan_revision_persistence::ControllerPlanRevisionPersistenceResult::Persisted {
            plan_id,
            ..
        } => plan_id,
        other => panic!("unexpected Controller revision persistence result: {other:?}"),
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
fn app_controller_adapter_persists_plan_and_review_before_canonical_apply() {
    let (directory, db, project) = setup();
    fs::create_dir_all(directory.path().join(".orc")).unwrap();
    fs::write(
        directory.path().join(".orc/engineering.md"),
        "Controller workflow test contract",
    )
    .unwrap();
    let workflow = db
        .start_controller_workflow(project, "app controller path", &automatic_policy())
        .unwrap();
    let mut planning = workflow.clone();
    planning.stage = WorkflowStage::Planner;
    let planning = db
        .commit_workflow_transition(
            &workflow,
            &planning,
            "test_controller_plan_stage",
            true,
            None,
            None,
        )
        .unwrap();
    let db_path = directory.path().join("orc.db");
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let mut runtime = AppControllerRuntime {
        responses: VecDeque::from([
            controller_plan_response("app controller path"),
            orc::local_runtime::LocalInferenceResponse::structured(
                "controller fixture",
                serde_json::json!({
                    "decision": "approve",
                    "details": "the bounded fixture plan is coherent",
                    "revision_feedback": null
                }),
            ),
        ]),
        calls: 0,
    };
    let completed = app
        .continue_workflow_with_controller_runtime(planning.id, &mut runtime)
        .unwrap();
    assert_eq!(
        completed.status,
        WorkflowStatus::Completed,
        "{}",
        completed.stop_reason.as_deref().unwrap_or("no reason")
    );
    assert_eq!(runtime.calls, 2);

    let state = app.workflow_state().unwrap();
    assert_eq!(state.plans.len(), 1);
    assert_eq!(
        state.plans[0].provenance,
        orc::storage::db::PlanProvenance::controller()
    );
    assert_eq!(state.plan_reviews.len(), 1);
    assert_eq!(
        state.plan_reviews[0].origin,
        orc::storage::db::PlanReviewOrigin::Controller
    );
    assert!(
        app.workflow_transitions(completed.id)
            .unwrap()
            .iter()
            .filter(|edge| { edge.edge == "plan_proposed" || edge.edge == "plan_reviewed" })
            .all(|edge| edge.provider_run_id.is_none())
    );
}

#[test]
fn app_controller_user_decision_persists_wait_and_resumes_review_after_reopen() {
    let (directory, db, project) = setup();
    fs::create_dir_all(directory.path().join(".orc")).unwrap();
    fs::write(
        directory.path().join(".orc/engineering.md"),
        "Controller user-decision workflow test contract",
    )
    .unwrap();
    let workflow = db
        .start_controller_workflow(project, "app controller question", &automatic_policy())
        .unwrap();
    let mut planning = workflow.clone();
    planning.stage = WorkflowStage::Planner;
    let planning = db
        .commit_workflow_transition(
            &workflow,
            &planning,
            "test_controller_plan_stage",
            true,
            None,
            None,
        )
        .unwrap();
    let db_path = directory.path().join("orc.db");
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let mut first_runtime = AppControllerRuntime {
        responses: VecDeque::from([
            controller_plan_response("app controller question"),
            orc::local_runtime::LocalInferenceResponse::structured(
                "controller fixture",
                serde_json::json!({
                    "decision": "operator_decision_required",
                    "details": "choose the deployment boundary",
                    "revision_feedback": null
                }),
            ),
        ]),
        calls: 0,
    };
    let waiting = app
        .continue_workflow_with_controller_runtime(planning.id, &mut first_runtime)
        .unwrap();
    assert_eq!(waiting.status, WorkflowStatus::WaitingUser);
    assert_eq!(waiting.stage, WorkflowStage::PlanReview);
    let plan_id = waiting.plan_id.unwrap();
    assert_eq!(
        app.workflow_state()
            .unwrap()
            .plans
            .iter()
            .find(|plan| plan.plan_id == plan_id)
            .unwrap()
            .status,
        orc::storage::db::PlanStatus::UnderReview
    );
    assert_eq!(app.workflow_state().unwrap().plan_reviews.len(), 1);
    drop(app);

    let reopened = OrcApp::open(&db_path, directory.path()).unwrap();
    let mut second_runtime = AppControllerRuntime {
        responses: VecDeque::from([orc::local_runtime::LocalInferenceResponse::structured(
            "controller fixture",
            serde_json::json!({
                "decision": "approve",
                "details": "the operator resolution is sufficient",
                "revision_feedback": null
            }),
        )]),
        calls: 0,
    };
    let completed = reopened
        .resolve_workflow_with_controller_runtime(
            waiting.id,
            "use the bounded deployment boundary",
            &mut second_runtime,
        )
        .unwrap();
    assert_eq!(
        completed.status,
        WorkflowStatus::Completed,
        "{}",
        completed.stop_reason.as_deref().unwrap_or("no reason")
    );
    assert_eq!(second_runtime.calls, 1);
    assert_eq!(reopened.workflow_state().unwrap().plan_reviews.len(), 2);
    assert_eq!(
        reopened
            .workflow_state()
            .unwrap()
            .plans
            .iter()
            .find(|plan| plan.plan_id == plan_id)
            .unwrap()
            .status,
        orc::storage::db::PlanStatus::Applied
    );
}

#[test]
fn controller_boundaries_recover_through_normal_continuation_without_runtime() {
    let (directory, db, project) = setup();
    let workflow = db
        .start_controller_workflow(
            project,
            "durable Controller boundaries",
            &automatic_policy(),
        )
        .unwrap();
    let mut planning = workflow.clone();
    planning.stage = WorkflowStage::Planner;
    let planning = db
        .commit_workflow_transition(
            &workflow,
            &planning,
            "test_controller_plan_stage",
            true,
            None,
            None,
        )
        .unwrap();
    let db_path = directory.path().join("orc.db");
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let plan_id = persist_workflow_plan(&app, planning.id, "durable Controller boundaries");
    persist_workflow_review(
        &app,
        planning.id,
        plan_id,
        None,
        orc::controller_plan_review::ControllerPlanReviewDecision::Approve,
    );
    let observer = Database::open(&db_path).unwrap();
    assert!(
        observer
            .controller_plan_for_workflow(planning.id, project, None)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        observer
            .controller_plan_review_for_workflow(planning.id, project, plan_id)
            .unwrap(),
        Some(orc::storage::db::PlanReviewDecision::Approve)
    );
    drop(observer);
    drop(app);

    let reopened = OrcApp::open(&db_path, directory.path()).unwrap();
    let completed = reopened.continue_workflow(planning.id).unwrap();
    assert_eq!(
        completed.status,
        WorkflowStatus::Completed,
        "{}",
        completed.stop_reason.as_deref().unwrap_or("no reason")
    );
    assert_eq!(reopened.workflow_state().unwrap().plans.len(), 1);
    assert_eq!(reopened.workflow_state().unwrap().plan_reviews.len(), 1);
    assert_eq!(
        reopened
            .workflow_state()
            .unwrap()
            .plans
            .iter()
            .find(|plan| plan.plan_id == plan_id)
            .unwrap()
            .status,
        orc::storage::db::PlanStatus::Applied
    );
}

#[test]
fn controller_recovery_is_workflow_bound_and_does_not_adopt_another_workflow_plan() {
    let (directory, db, project) = setup();
    let first = db
        .start_controller_workflow(project, "same objective", &automatic_policy())
        .unwrap();
    let mut first_planning = first.clone();
    first_planning.stage = WorkflowStage::Planner;
    let first_planning = db
        .commit_workflow_transition(
            &first,
            &first_planning,
            "test_controller_plan_stage",
            true,
            None,
            None,
        )
        .unwrap();
    let db_path = directory.path().join("orc.db");
    drop(db);
    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let first_plan_id = persist_workflow_plan(&app, first_planning.id, "same objective");
    drop(app);
    let db = Database::open(&db_path).unwrap();
    let second = db
        .start_controller_workflow(project, "same objective", &automatic_policy())
        .unwrap();
    let mut second_planning = second.clone();
    second_planning.stage = WorkflowStage::Planner;
    let second_planning = db
        .commit_workflow_transition(
            &second,
            &second_planning,
            "test_controller_plan_stage",
            true,
            None,
            None,
        )
        .unwrap();
    drop(db);

    let reopened = OrcApp::open(&db_path, directory.path()).unwrap();
    let stopped = reopened.continue_workflow(second_planning.id).unwrap();
    assert_eq!(stopped.status, WorkflowStatus::Blocked);
    assert_eq!(stopped.stage, WorkflowStage::Planner);
    assert_eq!(stopped.plan_id, None);
    assert_eq!(reopened.workflow_state().unwrap().plans.len(), 1);
    assert!(
        reopened
            .workflow_state()
            .unwrap()
            .plans
            .iter()
            .any(|plan| plan.plan_id == first_plan_id)
    );
    assert_eq!(reopened.workflow_state().unwrap().plan_reviews.len(), 0);
    assert_eq!(
        reopened
            .workflow_run(first_planning.id)
            .unwrap()
            .unwrap()
            .status,
        WorkflowStatus::Superseded
    );
}

#[test]
fn controller_revision_boundary_recovers_after_reopen_without_runtime() {
    let (directory, db, project) = setup();
    let workflow = db
        .start_controller_workflow(project, "revision boundary", &automatic_policy())
        .unwrap();
    let mut planning = workflow.clone();
    planning.stage = WorkflowStage::Planner;
    let planning = db
        .commit_workflow_transition(
            &workflow,
            &planning,
            "test_controller_plan_stage",
            true,
            None,
            None,
        )
        .unwrap();
    let db_path = directory.path().join("orc.db");
    drop(db);

    let app = OrcApp::open(&db_path, directory.path()).unwrap();
    let parent_id = persist_workflow_plan(&app, planning.id, "revision boundary");
    let review_id = persist_workflow_review(
        &app,
        planning.id,
        parent_id,
        None,
        orc::controller_plan_review::ControllerPlanReviewDecision::RevisePlan,
    );
    let mut review_stage = app.workflow_run(planning.id).unwrap().unwrap();
    review_stage.stage = WorkflowStage::PlanReview;
    review_stage.plan_id = Some(parent_id);
    let observer = Database::open(&db_path).unwrap();
    let review_stage = observer
        .commit_workflow_transition(
            &app.workflow_run(planning.id).unwrap().unwrap(),
            &review_stage,
            "test_controller_review_stage",
            true,
            None,
            None,
        )
        .unwrap();
    let actions = AppWorkflowActions::new(&app);
    let after_review = WorkflowEngine::new(&observer, &actions)
        .continue_one(review_stage.id)
        .unwrap();
    assert_eq!(after_review.stage, WorkflowStage::PlannerRevision);
    assert_eq!(after_review.plan_id, Some(parent_id));

    let parent = observer.get_plan(parent_id).unwrap().unwrap();
    let child_id =
        persist_workflow_revision(&app, planning.id, &parent, review_id, "revised boundary");
    persist_workflow_review(
        &app,
        planning.id,
        child_id,
        None,
        orc::controller_plan_review::ControllerPlanReviewDecision::Approve,
    );
    drop(observer);
    drop(app);

    let reopened = OrcApp::open(&db_path, directory.path()).unwrap();
    let completed = reopened.continue_workflow(planning.id).unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(reopened.workflow_state().unwrap().plans.len(), 2);
    assert_eq!(reopened.workflow_state().unwrap().plan_reviews.len(), 2);
    assert_eq!(
        reopened
            .workflow_state()
            .unwrap()
            .plans
            .iter()
            .find(|plan| plan.plan_id == child_id)
            .unwrap()
            .status,
        orc::storage::db::PlanStatus::Applied
    );
}

#[test]
fn controller_plan_path_routes_all_plan_boundaries_without_provider_lineage() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Plan);
    *actions.plan_reviews.lock().unwrap() =
        VecDeque::from([LeadDecisionKind::RevisePlan, LeadDecisionKind::Approve]);
    let mut runtime = NoopControllerRuntime;
    let run = WorkflowEngine::new(&db, &actions)
        .start_with_controller_runtime(project, "controller plan", automatic_policy(), &mut runtime)
        .unwrap();

    assert_eq!(run.status, WorkflowStatus::Completed);
    assert_eq!(*actions.controller_plan_calls.lock().unwrap(), 1);
    assert_eq!(*actions.controller_revision_calls.lock().unwrap(), 1);
    assert_eq!(*actions.controller_review_calls.lock().unwrap(), 2);
    assert_eq!(run.plan_revision_count, 1);
    let transitions = db.workflow_transitions(run.id).unwrap();
    for edge in transitions.iter().filter(|edge| {
        edge.edge == "plan_proposed" || edge.edge == "plan_revised" || edge.edge == "plan_reviewed"
    }) {
        assert!(!edge.deterministic);
        assert_eq!(edge.provider_run_id, None);
    }
    assert_eq!(
        transitions
            .iter()
            .filter(|edge| edge.edge == "plan_proposed" || edge.edge == "plan_revised")
            .count(),
        2
    );
    assert_eq!(
        transitions
            .iter()
            .filter(|edge| edge.edge == "plan_reviewed")
            .count(),
        2
    );
    assert!(
        db.list_agent_runs(project, usize::MAX)
            .unwrap()
            .into_iter()
            .flat_map(|run| db.provider_invocations(run.id).unwrap())
            .all(|invocation| invocation.purpose != "plan"
                && invocation.workflow_stage.as_deref() != Some("plan_review"))
    );
}

#[test]
fn controller_plan_path_preserves_configured_user_approval_gate() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Plan);
    let policy = WorkflowPolicy {
        plan_approval: orc::workflow::ApprovalPolicy::User,
        acceptance: AcceptancePolicy::Automatic,
        ..WorkflowPolicy::default()
    };
    let mut runtime = NoopControllerRuntime;
    let waiting = WorkflowEngine::new(&db, &actions)
        .start_with_controller_runtime(project, "approval gate", policy, &mut runtime)
        .unwrap();
    assert_eq!(waiting.status, WorkflowStatus::WaitingUser);
    assert_eq!(waiting.stage, WorkflowStage::ApplyPlan);
    assert_eq!(db.list_tasks_for_project(project).unwrap().len(), 0);

    let completed = WorkflowEngine::new(&db, &actions)
        .resolve_user_gate_with_controller_runtime(waiting.id, "approve", &mut runtime)
        .unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert!(!db.list_tasks_for_project(project).unwrap().is_empty());
}

#[test]
fn controller_plan_review_user_decision_reenters_controller_with_resolution() {
    let (_directory, db, project) = setup();
    let actions = FakeActions::new(&db, project, Intake::Plan);
    *actions.plan_reviews.lock().unwrap() = VecDeque::from([
        LeadDecisionKind::UserDecisionRequired,
        LeadDecisionKind::Approve,
    ]);
    let mut runtime = NoopControllerRuntime;
    let waiting = WorkflowEngine::new(&db, &actions)
        .start_with_controller_runtime(
            project,
            "controller question",
            automatic_policy(),
            &mut runtime,
        )
        .unwrap();
    assert_eq!(waiting.status, WorkflowStatus::WaitingUser);
    assert_eq!(waiting.stage, WorkflowStage::PlanReview);
    assert_eq!(*actions.controller_review_calls.lock().unwrap(), 1);

    let completed = WorkflowEngine::new(&db, &actions)
        .resolve_user_gate_with_controller_runtime(
            waiting.id,
            "use the bounded option",
            &mut runtime,
        )
        .unwrap();
    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(*actions.controller_review_calls.lock().unwrap(), 2);
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
        assert_eq!(invocations[0].escalation_reason, None);
        assert_eq!(invocations[0].escalation, None);
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
fn plan_revision_exhaustion_is_persisted_and_stops_provider_calls() {
    let (directory, db, project) = setup();
    let policy = WorkflowPolicy {
        max_plan_revisions: 2,
        ..automatic_policy()
    };
    let workflow_id = db
        .start_workflow(project, "plan without converging", &policy)
        .unwrap()
        .id;

    let before_restart = {
        let actions = FakeActions::new(&db, project, Intake::Plan);
        *actions.plan_reviews.lock().unwrap() = VecDeque::from([LeadDecisionKind::RevisePlan]);
        let engine = WorkflowEngine::new(&db, &actions);
        let mut current = db.get_workflow(workflow_id).unwrap().unwrap();
        for _ in 0..5 {
            current = engine.continue_one(workflow_id).unwrap();
        }
        current
    };
    assert_eq!(before_restart.status, WorkflowStatus::Running);
    assert_eq!(before_restart.stage, WorkflowStage::PlanReview);
    assert_eq!(before_restart.plan_revision_count, 1);
    drop(db);

    let reopened = Database::open(directory.path().join("orc.db")).unwrap();
    let actions = FakeActions::new(&reopened, project, Intake::Plan);
    *actions.plan_reviews.lock().unwrap() =
        VecDeque::from([LeadDecisionKind::RevisePlan, LeadDecisionKind::RevisePlan]);
    let engine = WorkflowEngine::new(&reopened, &actions);
    let stopped = engine.continue_run(workflow_id).unwrap();

    assert_eq!(stopped.status, WorkflowStatus::NonConvergent);
    assert_eq!(stopped.stage, WorkflowStage::PlannerRevision);
    assert_eq!(stopped.plan_revision_count, policy.max_plan_revisions);
    assert_eq!(
        stopped.stop_reason.as_deref(),
        Some("plan revision limit exhausted")
    );
    assert!(reopened.list_tasks_for_project(project).unwrap().is_empty());

    let invocations = reopened
        .list_agent_runs(project, usize::MAX)
        .unwrap()
        .into_iter()
        .flat_map(|run| reopened.provider_invocations(run.id).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        invocations
            .iter()
            .filter(|invocation| {
                invocation.purpose == "plan"
                    && invocation.workflow_stage.as_deref() == Some("planner")
            })
            .count(),
        1
    );
    assert_eq!(
        invocations
            .iter()
            .filter(|invocation| {
                invocation.purpose == "plan"
                    && invocation.workflow_stage.as_deref() == Some("planner_revision")
            })
            .count(),
        policy.max_plan_revisions
    );
    assert_eq!(
        invocations
            .iter()
            .filter(|invocation| {
                invocation.purpose == "lead"
                    && invocation.workflow_stage.as_deref() == Some("plan_review")
            })
            .count(),
        policy.max_plan_revisions + 1
    );

    let invocation_count = invocations.len();
    let unchanged = engine.continue_run(workflow_id).unwrap();
    assert_eq!(unchanged.status, WorkflowStatus::NonConvergent);
    let after_terminal_count = reopened
        .list_agent_runs(project, usize::MAX)
        .unwrap()
        .into_iter()
        .flat_map(|run| reopened.provider_invocations(run.id).unwrap())
        .count();
    assert_eq!(after_terminal_count, invocation_count);
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
        .start_provider_invocation_with_resolution(
            provider_run,
            "implementation",
            1,
            &fake_resolution("implementation"),
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
fn acceptance_gate_accepts_after_restart_without_provider_invocation() {
    let (directory, db, project) = setup();
    let waiting = {
        let actions = FakeActions::new(&db, project, Intake::Direct);
        WorkflowEngine::new(&db, &actions)
            .start(
                project,
                "approval survives restart",
                WorkflowPolicy::default(),
            )
            .unwrap()
    };
    assert_eq!(waiting.status, WorkflowStatus::AcceptanceReady);
    assert_eq!(waiting.stage, WorkflowStage::Acceptance);
    let task_id = waiting.current_task_id.clone().unwrap();
    let invocations_before = db
        .list_agent_runs(project, usize::MAX)
        .unwrap()
        .into_iter()
        .flat_map(|run| db.provider_invocations(run.id).unwrap())
        .count();
    drop(db);

    let reopened = Database::open(directory.path().join("orc.db")).unwrap();
    let actions = FakeActions::new(&reopened, project, Intake::Direct);
    let completed = WorkflowEngine::new(&reopened, &actions)
        .resolve_user_gate(waiting.id, "accept")
        .unwrap();

    assert_eq!(completed.status, WorkflowStatus::Completed);
    assert_eq!(completed.stage, WorkflowStage::Done);
    assert_eq!(
        reopened.get_task(&task_id).unwrap().unwrap().status,
        TaskStatus::Done
    );
    let invocations_after = reopened
        .list_agent_runs(project, usize::MAX)
        .unwrap()
        .into_iter()
        .flat_map(|run| reopened.provider_invocations(run.id).unwrap())
        .count();
    assert_eq!(invocations_after, invocations_before);
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
