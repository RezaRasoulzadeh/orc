//! Opt-in Qwen3 evaluation for the read-only Controller decision corpus.
//!
//! This test is ignored and requires `--features llama-cpp` plus a local
//! user-supplied Qwen3 8B GGUF path. It emits typed per-scenario outcomes and
//! an aggregate count; it never executes a recommended action.

#![cfg(feature = "llama-cpp")]

use orc::controller_evaluation::{
    ControllerEvaluationReport, ControllerParseDiagnostic, evaluate_scenarios,
    parse_structured_output, representative_scenarios,
};
use orc::local_runtime::{
    LlamaCppRuntime, LocalInferenceError, LocalInferenceParameters, LocalInferenceRequest,
    LocalInferenceResponse, LocalInferenceRuntime, LocalRuntimeConfig,
};
use std::env;
use std::path::PathBuf;

struct JsonEvaluationRuntime {
    inner: LlamaCppRuntime,
    parse_diagnostics: Vec<Option<ControllerParseDiagnostic>>,
}

impl LocalInferenceRuntime for JsonEvaluationRuntime {
    fn infer(
        &mut self,
        request: &LocalInferenceRequest,
    ) -> Result<LocalInferenceResponse, LocalInferenceError> {
        let prompt = format!(
            "{}\n\nFor this evaluation, return only one JSON object with these fields: \
`suggested_next_step` (one canonical next-step value or null), \
`decision_class` (`action` or `operator_decision`), and `rationale` (short string). \
Use `operator_decision` with a null next step when the facts are ambiguous or \
inconsistent. Do not execute or claim to execute any action.",
            request.prompt
        );
        let parameters = LocalInferenceParameters {
            max_output_tokens: 256,
            temperature: 0.0,
            top_p: 1.0,
            stop_sequences: request.parameters.stop_sequences.clone(),
            response_format: request.parameters.response_format.clone(),
        };
        let request = match LocalInferenceRequest::new(prompt, parameters) {
            Ok(request) => request,
            Err(error) => {
                self.parse_diagnostics.push(None);
                return Err(error);
            }
        };
        let mut response = match self.inner.infer(&request) {
            Ok(response) => response,
            Err(error) => {
                self.parse_diagnostics.push(None);
                return Err(error);
            }
        };
        match parse_structured_output(&response.text) {
            Ok(structured_output) => {
                self.parse_diagnostics.push(None);
                response.structured_output = Some(structured_output);
            }
            Err(parse_error) => {
                self.parse_diagnostics
                    .push(Some(ControllerParseDiagnostic::new(
                        response.text.clone(),
                        parse_error,
                    )));
                response.structured_output = None;
            }
        }
        Ok(response)
    }
}

fn print_report(report: &ControllerEvaluationReport) {
    for scenario in &report.scenarios {
        println!(
            "{} expected_class={} expected={} observed={} result={:?}",
            scenario.scenario_id,
            scenario.expected_action_class.as_str(),
            scenario.expected_decision,
            scenario.observed_decision,
            scenario.result,
        );
        if let Some(parse_error) = scenario.parse_error.as_deref() {
            println!("  parse_error={parse_error:?}");
            if let Some(raw_model_output) = scenario.raw_model_output.as_deref() {
                println!("  raw_model_output={raw_model_output:?}");
            }
        } else {
            if let Some(rationale) = scenario.rationale.as_deref() {
                println!("  rationale={rationale:?}");
            }
            if let Some(confidence) = scenario.confidence {
                println!("  confidence={confidence}");
            }
        }
        if let Some(error) = scenario.error.as_deref() {
            println!("  error={error:?}");
        }
    }
    println!(
        "aggregate: {} passed; {} failed",
        report.passed, report.failed
    );
}

#[test]
#[ignore = "requires a user-supplied Qwen3 8B GGUF; see docs/orc-next/tasks/M02-002.md"]
fn qwen3_evaluates_read_only_controller_decisions() {
    let model_path = env::var_os("ORC_QWEN3_GGUF")
        .map(PathBuf::from)
        .expect("set ORC_QWEN3_GGUF to a local Qwen3 8B GGUF file");
    let config = LocalRuntimeConfig::new(model_path).with_context_tokens(8192);
    let runtime = LlamaCppRuntime::from_config(&config).expect("load local Qwen3 GGUF");
    let scenarios = representative_scenarios().expect("representative scenarios are valid");
    let mut runtime = JsonEvaluationRuntime {
        inner: runtime,
        parse_diagnostics: Vec::with_capacity(scenarios.len()),
    };
    let report = evaluate_scenarios(&scenarios, &mut runtime).expect("evaluation report");
    let mut report = report;
    for (scenario, diagnostic) in scenarios.iter().zip(runtime.parse_diagnostics) {
        if let Some(diagnostic) = diagnostic {
            assert!(report.record_parse_failure(&scenario.id, diagnostic));
        }
    }
    print_report(&report);
    assert!(
        report.is_success(),
        "one or more Controller scenarios failed"
    );
}
