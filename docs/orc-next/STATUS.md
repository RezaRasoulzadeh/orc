# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M02 — Read-only Controller

**Current task:** M02-003 — Enforce reliable structured Controller output (In progress; implementation complete, real-model evaluation pending)

**Last completed:** M02-002 — Read-only Controller decision quality evaluation

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable, read-only local inference seam; native backend details remain outside Controller/domain types. The optional `llama-cpp` feature implements the first adapter in `src/local_runtime/llama_cpp.rs`.
- Controller recommendations use a model-independent JSON Schema request; the
  llama.cpp adapter applies native grammar-constrained sampling and strict
  full-value parsing, retaining raw output on structured parse failure.
- M01-002 was source-reviewed as PASS. The supplied `Qwen3-8B-Q4_K_M.gguf` smoke passed end-to-end through `LocalInferenceRuntime` → `LlamaCppRuntime` → llama.cpp on CPU; Vulkan/GPU optimization remains separate from M01-002.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

M02-002 remains recorded with strict structured compliance `4/7` and semantic
action selection `7/7`; the strict result was not reinterpreted as contract
compliance. M02-003 addresses that protocol gap without stripping trailing
model output.

Run the opt-in Qwen3 evaluation to verify that all seven unchanged M02-002
scenarios satisfy both semantic expectations and the strict JSON contract:

```text
ORC_QWEN3_GGUF=~/models/qwen3/Qwen3-8B-Q4_K_M.gguf \
  cargo test --features llama-cpp --test controller_evaluation_smoke -- --ignored --nocapture
```

M02-003's implementation now uses native JSON Schema → llama.cpp grammar
constrained decoding and retains structured-output diagnostics. The required
real-model run is intentionally still pending and no model weights are part
of the repository. A focused native smoke also confirmed and fixed the
sampler-contract crash caused by accepting each token twice; the final
seven-scenario evaluation remains pending while the local model choice is
being replaced.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
