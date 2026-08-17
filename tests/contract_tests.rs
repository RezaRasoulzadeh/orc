use orc::contract;
use orc::task::{Task, TaskPriority, TaskStatus};
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
        required_capabilities: Vec::new(),
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
        required_capabilities: Vec::new(),
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
