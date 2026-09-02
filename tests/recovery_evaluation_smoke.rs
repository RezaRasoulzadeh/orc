//! Opt-in Qwen3 evaluation for the read-only M04-002 recovery corpus.
//!
//! This is intentionally ignored during normal validation. It uses the
//! existing local runtime configuration and only evaluates typed output; it
//! never authorizes or executes recovery.

#![cfg(feature = "llama-cpp")]

use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceRuntime, LocalRuntimeConfig,
};
use orc::recovery_controller::{
    RecoveryEvaluationReport, evaluate_recovery_scenarios, representative_recovery_scenarios,
};
use std::env;
use std::path::PathBuf;

struct RecoveryQwenRuntime {
    inner: LlamaCppRuntime,
}

impl LocalInferenceRuntime for RecoveryQwenRuntime {
    fn infer(
        &mut self,
        request: &orc::local_runtime::LocalInferenceRequest,
    ) -> Result<orc::local_runtime::LocalInferenceResponse, LocalInferenceError> {
        self.inner.infer(request)
    }
}

fn print_report(report: &RecoveryEvaluationReport) {
    for scenario in &report.scenarios {
        println!(
            "{} strict_contract={} expected={} observed={:?} result={:?}",
            scenario.scenario_id,
            scenario.strict_contract,
            serde_json::to_string(&scenario.expected).unwrap(),
            scenario.observed,
            scenario.result,
        );
    }
    println!(
        "strict structured contract: {} passed; {} failed",
        report.strict_passed, report.strict_failed
    );
    println!(
        "semantic decision result: {} passed; {} failed",
        report.semantic_passed, report.semantic_failed
    );
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M04-002.md"]
fn qwen3_evaluates_read_only_recovery_decisions() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let scenarios = representative_recovery_scenarios();
    let mut runtime = RecoveryQwenRuntime { inner: runtime };
    let report = evaluate_recovery_scenarios(&scenarios, &mut runtime).expect("evaluation report");
    print_report(&report);
    assert!(report.is_success(), "one or more recovery scenarios failed");
}
