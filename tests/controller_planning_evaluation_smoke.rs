//! Opt-in real-model evaluation for the read-only Controller planning seam.
//!
//! This is intentionally ignored and requires a local Qwen3 GGUF. It only
//! validates the strict typed planning contract; it never persists or applies
//! the proposed plan.

#![cfg(feature = "llama-cpp")]

use orc::controller_planning::{
    ControllerPlanResult, ControllerPlanningBuilder, ControllerPlanningRequest,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRequest, LocalInferenceResponse,
    LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::protocol::{PROTOCOL_VERSION, PlanResponseSchema, PlanningRequest};
use std::env;
use std::path::PathBuf;

struct JsonRuntime {
    inner: LlamaCppRuntime,
}

impl LocalInferenceRuntime for JsonRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        let mut response = self.inner.infer(request)?;
        let value =
            serde_json::from_str::<serde_json::Value>(response.text.trim()).map_err(|error| {
                LocalInferenceError::InvalidStructuredOutput {
                    raw_output: response.text.clone(),
                    parse_error: error.to_string(),
                }
            })?;
        response.structured_output = Some(value);
        Ok(response)
    }
}

fn planning_request() -> PlanningRequest {
    PlanningRequest {
        protocol_version: PROTOCOL_VERSION,
        kind: "project_plan".into(),
        project: Some(orc::protocol::ReportProject {
            name: "controller-planning-evaluation".into(),
            repository: "omitted-from-controller-request".into(),
            branch: None,
            commit: None,
        }),
        engineering_contract: "Keep the proposal bounded and read-only.".into(),
        objective: "Propose one small, reviewable engineering task.".into(),
        constraints: vec!["Do not apply or persist the plan.".into()],
        target_platforms: vec![],
        stack: vec!["Rust".into()],
        non_goals: vec!["Task creation".into(), "Dispatch".into()],
        deliverables: vec!["A typed PlanResponse proposal".into()],
        definition_of_done: vec!["The proposal is bounded and reviewable.".into()],
        response_schema: PlanResponseSchema::v1(),
        role_boundaries: vec!["Controller proposes only.".into()],
        planning_constraints: vec![],
        approval_requirements: vec!["Operator approval is required.".into()],
        current_state: None,
        full_report: None,
        discovery_snapshot: None,
    }
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M05-001.md"]
fn qwen3_evaluates_read_only_controller_planning_contract() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let bounded = ControllerPlanningRequest::from_canonical(&planning_request())
        .expect("canonical request is bounded");
    let mut runtime = JsonRuntime { inner: runtime };
    let result: Result<ControllerPlanResult, _> =
        ControllerPlanningBuilder::new().propose(&bounded, &mut runtime);
    match result {
        Ok(result) => {
            println!("strict_contract=true result=Pass");
            assert!(result.validate().is_ok());
        }
        Err(error) => panic!("strict_contract=false result=Fail error={error}"),
    }
}
