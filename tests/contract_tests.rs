use orc::contract;
use orc::protocol::{PROTOCOL_VERSION, PlanResponse, PlannedTask};
use orc::task::{Task, TaskPriority, TaskScopeMode, TaskStatus};
use tempfile::tempdir;

#[test]
fn contract_loading_succeeds() {
    let dir = tempdir().unwrap();
    let contract_path = dir.path().join("engineering.md");
    std::fs::write(&contract_path, "# Test Contract\nTest content").unwrap();

    let result = contract::load_contract(&contract_path);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "# Test Contract\nTest content");
}

#[test]
fn default_contract_covers_constitutional_requirements() {
    let contract = orc::contract::DEFAULT_ENGINEERING_CONTRACT;
    for requirement in [
        "existing Orc application, orchestration, and shared core APIs",
        "deterministic behavioral tests",
        "provider-independent",
        "foreign keys",
        "transactions",
        "Migrations must be incremental",
        "unsafe `Send`/`Sync` ownership hacks",
        "Linux, macOS, and Windows",
        "exact revision",
        "cargo clippy --all-targets -- -D warnings",
    ] {
        assert!(
            contract.contains(requirement),
            "missing requirement: {requirement}"
        );
    }
}

#[test]
fn missing_contract_produces_error() {
    let dir = tempdir().unwrap();
    let contract_path = dir.path().join("nonexistent.md");

    let result = contract::load_contract(&contract_path);
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("failed to load engineering contract"));
}

#[test]
fn build_worker_prompt_contains_engineering_contract() {
    let contract = "# Engineering Contract\nKey principles: Be explicit.";
    let task = Task {
        id: "T-0001".to_string(),
        title: "Implement feature".to_string(),
        objective: "Add new feature X".to_string(),
        role: "backend-engineer".to_string(),
        priority: TaskPriority::Normal,
        status: TaskStatus::Ready,
        cancellation_reason: None,
        required_capabilities: Vec::new(),
        scope_mode: None,
        context_files: Vec::new(),
        expected_changes: Vec::new(),
        reasoning_effort: None,
        effort_reason: None,
        risk_factors: Vec::new(),
    };

    let prompt = orc::agent::build_worker_prompt_for_testing(contract, "testproj", &task);

    // Verify contract is included
    assert!(prompt.contains(contract));
    assert!(prompt.contains("# Engineering Contract"));

    // Verify contract appears before task info
    let contract_pos = prompt.find(contract).unwrap();
    let task_title_pos = prompt.find("Implement feature").unwrap();
    assert!(contract_pos < task_title_pos);
}

#[test]
fn generated_worker_prompt_still_contains_task_information() {
    let contract = "# Contract";
    let task = Task {
        id: "T-0042".to_string(),
        title: "Do the thing".to_string(),
        objective: "Complete the objective".to_string(),
        role: "lead-engineer".to_string(),
        priority: TaskPriority::High,
        status: TaskStatus::Active,
        cancellation_reason: None,
        required_capabilities: Vec::new(),
        scope_mode: None,
        context_files: Vec::new(),
        expected_changes: Vec::new(),
        reasoning_effort: None,
        effort_reason: None,
        risk_factors: Vec::new(),
    };

    let prompt = orc::agent::build_worker_prompt_for_testing(contract, "myproject", &task);

    // Verify all task-related information is present
    assert!(prompt.contains("Project: myproject"));
    assert!(prompt.contains("Task ID: T-0042"));
    assert!(prompt.contains("Title: Do the thing"));
    assert!(prompt.contains("Objective: Complete the objective"));
    assert!(prompt.contains("Role: lead-engineer"));

    // Verify instructions are still there
    assert!(prompt.contains("Inspect the repository"));
    assert!(prompt.contains("implement ONLY the changes required"));
}

#[test]
fn targeted_prompt_guidance_is_scoped_and_optional() {
    let mut task = Task {
        id: "T-0002".into(),
        title: "Targeted".into(),
        objective: "Implement it".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        status: TaskStatus::Ready,
        cancellation_reason: None,
        required_capabilities: Vec::new(),
        scope_mode: Some(orc::task::TaskScopeMode::Focused),
        context_files: vec!["src/review.rs".into(), "src/agent.rs".into()],
        expected_changes: vec!["src/review.rs".into()],
        reasoning_effort: None,
        effort_reason: None,
        risk_factors: Vec::new(),
    };
    let prompt = orc::agent::build_worker_prompt_for_testing("# Contract", "p", &task);
    assert!(prompt.contains("Read these files first:"));
    assert!(prompt.contains("- src/review.rs"));
    assert!(prompt.contains("Expected changes:"));
    task.scope_mode = Some(orc::task::TaskScopeMode::Project);
    let project_prompt = orc::agent::build_worker_prompt_for_testing("# Contract", "p", &task);
    assert!(project_prompt.contains("broader repository inspection is allowed"));
}

fn planned_task() -> PlannedTask {
    PlannedTask {
        local_id: "task-1".into(),
        title: "Task".into(),
        objective: "Do task".into(),
        role: "developer".into(),
        priority: TaskPriority::Normal,
        depends_on: Vec::new(),
        capabilities: Vec::new(),
        scope_mode: Some(TaskScopeMode::Focused),
        context_files: vec!["src/lib.rs".into()],
        expected_changes: vec!["src/lib.rs".into()],
        unchanged: vec!["other modules".into()],
        acceptance_criteria: vec!["task behavior is complete".into()],
        required_tests: vec!["production test".into()],
        validation: vec!["cargo test".into()],
        execution_hints: Default::default(),
        risk_factors: vec![],
    }
}

fn plan_with_task(task: PlannedTask) -> PlanResponse {
    PlanResponse {
        protocol_version: PROTOCOL_VERSION,
        objective: "Plan".into(),
        assumptions: Vec::new(),
        risks: Vec::new(),
        questions: Vec::new(),
        tasks: vec![task],
    }
}

#[test]
fn plan_rejects_absolute_context_file() {
    let mut task = planned_task();
    task.context_files = vec!["/tmp/file.rs".into()];
    let error = plan_with_task(task).validate().unwrap_err().to_string();
    assert!(error.contains("context_files") && error.contains("absolute"));
}

#[test]
fn plan_rejects_traversal_expected_change() {
    let mut task = planned_task();
    task.expected_changes = vec!["src/../file.rs".into()];
    let error = plan_with_task(task).validate().unwrap_err().to_string();
    assert!(error.contains("expected_changes") && error.contains(".."));
}

#[test]
fn targeted_scope_requires_context_files() {
    let mut task = planned_task();
    task.context_files.clear();
    let error = plan_with_task(task).validate().unwrap_err().to_string();
    assert!(error.contains("targeted scope") && error.contains("context file"));
}

#[test]
fn valid_project_scope_without_context_still_passes() {
    let mut task = planned_task();
    task.scope_mode = Some(TaskScopeMode::Project);
    task.context_files.clear();
    assert!(plan_with_task(task).validate().is_ok());
}
