# Orc Next Status

**Architecture:** Orc Next / local Controller + deterministic kernel

**Current milestone:** M03 — Typed Controller tools

**Current task:** M03-002 — Execute explicitly authorized Controller intents (not yet decomposed)

**Last completed:** M03-001 — Define typed Controller action intents and legality boundary

**Blocked by:** Nothing

## Current decisions

- Local Controller from phase one.
- Initial model/runtime target: Qwen3 8B through llama.cpp/GGUF.
- Qwen/llama.cpp is an implementation choice, not a domain dependency; the runtime boundary must remain replaceable.
- `ProjectOperations` is the primary existing provider-independent observation seam.
- `OrcApp` is the existing canonical application/mutation seam.
- `src/local_runtime.rs` is the replaceable, read-only local inference seam; native backend details remain outside Controller/domain types. The optional `llama-cpp` feature implements the first adapter in `src/local_runtime/llama_cpp.rs`.
- Controller recommendations use a model-independent JSON Schema request; the llama.cpp adapter applies native grammar-constrained sampling and strict full-value parsing, retaining raw output on structured parse failure.
- Controller state now carries an explicit canonical operational-consistency observation so derived next-step data cannot silently override contradictory lifecycle facts.
- M02 is complete: the final unchanged seven-scenario Qwen3 evaluation achieved `7/7` semantic decisions and `7/7` strict structured-contract compliance.
- Memory is explicit Orc data, separate from model weights.
- Lead/Planner reasoning will move into Controller after the Controller foundation is proven; durable useful Plan/decision data is preserved.
- Preserve Dispatch/Review/Revise/Accept execution primitives and deterministic validation truth.
- Rust/native runtime; avoid Python.

## Immediate next action

Define M03-002 around explicit, re-checked execution of authorized intents.

M03-001 is complete. The read-only `ControllerActionIntent` boundary supports
dispatch, semantic review, revise and accept; canonical `ProjectOperations`
legality inspection returns bounded typed Allowed/Rejected facts without
executing or persisting an action. M03-002 remains intentionally undefined;
its first design concern is an explicit permission/confirmation and a
mutation-boundary legality re-check, not treating inspection as authorization.

M02-003 final acceptance evidence:

- final Luna + High source review: `PASS`;
- semantic decision quality: `7/7`;
- strict structured-contract compliance: `7/7`;
- real-model evaluation: `1 passed, 0 failed`, `189.80s`;
- latest deterministic validation: 293 normal lib tests and 299 llama-cpp-feature lib tests passed, with normal/feature clippy, fmt, and diff check passing.

See `M00-REPOSITORY-MAP.md` for the repository-grounded migration map.
