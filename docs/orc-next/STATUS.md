# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M02 — Read-only Controller

**Current task:** M02-003 — Enforce reliable structured Controller output (In progress; strict output 7/7, semantic decision quality 6/7)

**Last completed:** M02-002 — Read-only Controller decision quality evaluation

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable, read-only local inference seam; native backend details remain outside Controller/domain types. The optional `llama-cpp` feature implements the first adapter in `src/local_runtime/llama_cpp.rs`.
- Controller recommendations use a model-independent JSON Schema request; the llama.cpp adapter applies native grammar-constrained sampling and strict full-value parsing, retaining raw output on structured parse failure.
- M01-002 was source-reviewed as PASS. The supplied `Qwen3-8B-Q4_K_M.gguf` smoke passed end-to-end through `LocalInferenceRuntime` → `LlamaCppRuntime` → llama.cpp on CPU; Vulkan/GPU optimization remains separate from M01-002.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

M02-003's native grammar path is now proven reliable on the seven-scenario real-model evaluation: strict structured-contract compliance is `7/7`, and the sampler double-acceptance crash is fixed.

The same run produced semantic decision quality `6/7`. The sole failure is `review-revise`: a canonical `RevisionRequired` state with current `REVISE` evidence and `next_step=revise` was returned as `operator_decision`. The model's rationale itself said revision was necessary, so this is a decision-contract ambiguity rather than a structured-output failure.

Refine the generic Controller prompt/decision contract so clear canonical actionable state maps to an action while `operator_decision` remains reserved for genuinely ambiguous/inconsistent/non-actionable state. Do not hardcode the scenario, force-copy `next_step`, weaken expectations, or change the already-working grammar/schema path. After deterministic validation and source review, rerun the seven-scenario evaluation once. M02-003 acceptance still requires `7/7` semantic and `7/7` strict compliance.

M03-001 is defined but must not start until M02-003 is accepted.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
